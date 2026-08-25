//! Config file: `$XDG_CONFIG_HOME/sleepless/config.toml` (usually
//! `~/.config/sleepless/config.toml`).
//!
//! Everything is optional and CLI flags win over the file. Broken entries are
//! skipped with a warning shown in the TUI, never fatal — a bad config must
//! not stop the machine from staying awake.

use crate::art::{Splash, SplashSource};
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
pub struct FileConfig {
    /// "always" or "plugged-only".
    pub mode: Option<String>,
    pub inhibit_lid: Option<bool>,
    pub tray: Option<bool>,
    pub pulse: Option<bool>,
    pub why: Option<String>,
    /// Name of the splash to show at startup.
    pub splash: Option<String>,
    #[serde(default)]
    pub splashes: Vec<SplashDef>,
}

#[derive(Debug, Deserialize)]
pub struct SplashDef {
    pub name: Option<String>,
    /// Lines rendered with the built-in block font (A-Z, 0-9, `'-!?.`).
    pub text: Option<Vec<String>>,
    /// Verbatim ASCII art (mutually exclusive with `text`).
    pub art: Option<String>,
    /// Path to a file with verbatim ASCII art; leading `~/` is expanded.
    pub art_file: Option<String>,
    pub color: Option<String>,
    pub pulse_color: Option<String>,
    pub paused_color: Option<String>,
}

pub struct Loaded {
    pub file: FileConfig,
    pub warnings: Vec<String>,
}

/// Runtime changes (mode/lid toggles, chosen splash), saved as they happen and
/// restored on the next launch. Kept in `$XDG_STATE_HOME/sleepless/state.toml`
/// (usually `~/.local/state/...`), separate from the hand-edited config.toml,
/// which is never rewritten. Precedence: CLI flags > state > config.toml.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// "always" or "plugged-only".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inhibit_lid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub splash: Option<String>,
}

/// `$XDG_STATE_HOME` (Unix) or `%LOCALAPPDATA%` (Windows).
///
/// Returns `None` when there is nowhere sensible to write. It must never fall
/// back to the current directory: on Windows `$HOME` is normally unset, and the
/// old fallback silently scattered `sleepless/state.toml` into whatever directory
/// the user happened to launch from.
pub fn state_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = resolve_base(
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        None,
        &[],
    );
    #[cfg(not(windows))]
    let base = resolve_base(
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
        home_dir(),
        &[".local", "state"],
    );
    Some(base?.join("sleepless").join("state.toml"))
}

/// Pure part of the path rules, so they can be tested without mutating the
/// process environment (which is `unsafe` in edition 2024 and racy under a
/// parallel test runner).
///
/// A relative `explicit` is ignored: the XDG spec says a non-absolute
/// `XDG_*_HOME` must be treated as unset.
fn resolve_base(
    explicit: Option<PathBuf>,
    home: Option<PathBuf>,
    home_rel: &[&str],
) -> Option<PathBuf> {
    explicit
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| home_rel.iter().fold(h, |acc, c| acc.join(c))))
}

pub fn load_state() -> State {
    match state_path() {
        Some(p) => load_state_from(&p),
        None => State::default(),
    }
}

fn load_state_from(path: &Path) -> State {
    // Like the config: an unreadable or broken state file is never fatal.
    let mut state: State = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default();
    if let Some(m) = &state.mode
        && !matches!(m.as_str(), "always" | "plugged-only")
    {
        state.mode = None;
    }
    state
}

pub fn save_state(state: &State) -> std::io::Result<()> {
    let path = state_path().ok_or_else(|| {
        std::io::Error::other("no state directory (set XDG_STATE_HOME, HOME or LOCALAPPDATA)")
    })?;
    save_state_to(&path, state)
}

fn save_state_to(path: &Path, state: &State) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = toml::to_string(state).map_err(std::io::Error::other)?;
    // Write-then-rename so a crash mid-write can't leave a truncated file.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

/// `$XDG_CONFIG_HOME` (Unix) or `%APPDATA%` (Windows). See [`state_path`] for
/// why this is an `Option` rather than a fallback to `.`.
///
/// macOS deliberately keeps the XDG layout instead of `~/Library/Application
/// Support`: this is a terminal program and `~/.config` is where its users look.
pub fn config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = resolve_base(std::env::var_os("APPDATA").map(PathBuf::from), None, &[]);
    #[cfg(not(windows))]
    let base = resolve_base(
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        home_dir(),
        &[".config"],
    );
    Some(base?.join("sleepless").join("config.toml"))
}

pub fn load() -> Loaded {
    let mut warnings = Vec::new();
    let Some(path) = config_path() else {
        return Loaded {
            file: FileConfig::default(),
            warnings,
        };
    };
    let file = match std::fs::read_to_string(&path) {
        Err(_) => FileConfig::default(), // no config file is fine
        Ok(s) => match toml::from_str(&s) {
            Ok(c) => c,
            Err(e) => {
                warnings.push(format!("config: {}: {e}", path.display()));
                FileConfig::default()
            }
        },
    };
    if let Some(m) = &file.mode
        && !matches!(m.as_str(), "always" | "plugged-only")
    {
        warnings.push(format!(
            "config: unknown mode {m:?} (use \"always\" or \"plugged-only\")"
        ));
    }
    Loaded { file, warnings }
}

/// Built-in splashes followed by the ones from the config file.
pub fn build_splashes(file: &FileConfig, warnings: &mut Vec<String>) -> Vec<Splash> {
    let mut out = builtin_splashes();
    for (i, def) in file.splashes.iter().enumerate() {
        let name = def
            .name
            .clone()
            .unwrap_or_else(|| format!("splash-{}", i + 1));
        let source = match (&def.text, &def.art, &def.art_file) {
            (Some(text), None, None) => SplashSource::Text(text.clone()),
            (None, Some(art), None) => {
                SplashSource::Art(art.trim_matches('\n').lines().map(str::to_string).collect())
            }
            (None, None, Some(path)) => match std::fs::read_to_string(expand_home(path)) {
                Ok(s) => {
                    SplashSource::Art(s.trim_matches('\n').lines().map(str::to_string).collect())
                }
                Err(e) => {
                    warnings.push(format!("config: splash {name:?}: cannot read {path}: {e}"));
                    continue;
                }
            },
            _ => {
                warnings.push(format!(
                    "config: splash {name:?} needs exactly one of text / art / art_file — skipped"
                ));
                continue;
            }
        };
        let color = resolve_color(&def.color, "color", Color::Green, &name, warnings);
        let pulse_color = resolve_color(
            &def.pulse_color,
            "pulse_color",
            Color::LightGreen,
            &name,
            warnings,
        );
        let paused_color = resolve_color(
            &def.paused_color,
            "paused_color",
            Color::DarkGray,
            &name,
            warnings,
        );
        out.push(Splash {
            name,
            source,
            color,
            pulse_color,
            paused_color,
        });
    }
    out
}

fn resolve_color(
    field: &Option<String>,
    what: &str,
    default: Color,
    splash: &str,
    warnings: &mut Vec<String>,
) -> Color {
    match field {
        None => default,
        Some(s) => match parse_color(s) {
            Some(c) => c,
            None => {
                warnings.push(format!("config: splash {splash:?}: unknown {what} {s:?}"));
                default
            }
        },
    }
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(p)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Named terminal colors or `#rrggbb`.
pub fn parse_color(s: &str) -> Option<Color> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix('#') {
        if hex.len() == 6
            && let Ok(v) = u32::from_str_radix(hex, 16)
        {
            return Some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
        }
        return None;
    }
    let c = match t.to_ascii_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return None,
    };
    Some(c)
}

pub fn builtin_splashes() -> Vec<Splash> {
    vec![
        Splash {
            name: "default".into(),
            source: SplashSource::Text(vec!["I CAN'T".into(), "GET NO".into(), "SLEEP".into()]),
            color: Color::Green,
            pulse_color: Color::LightGreen,
            paused_color: Color::DarkGray,
        },
        Splash {
            name: "brooklyn".into(),
            source: SplashSource::Text(vec!["NO SLEEP".into(), "TILL".into(), "BROOKLYN".into()]),
            color: Color::Cyan,
            pulse_color: Color::LightCyan,
            paused_color: Color::DarkGray,
        },
        Splash {
            name: "coffee".into(),
            source: SplashSource::Art(
                COFFEE
                    .trim_matches('\n')
                    .lines()
                    .map(str::to_string)
                    .collect(),
            ),
            color: Color::Yellow,
            pulse_color: Color::LightYellow,
            paused_color: Color::DarkGray,
        },
    ]
}

const COFFEE: &str = r"
         ) )
        ( (
         ) )
     .--------.
     |        |]
     |        |
     '.______.'
   C A F F E I N E
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let c: FileConfig = toml::from_str(
            r##"
mode = "plugged-only"
inhibit_lid = true
pulse = false
splash = "coffee"
why = "rendering"

[[splashes]]
name = "hello"
text = ["HELLO", "WORLD"]
color = "#ff00ff"

[[splashes]]
name = "cat"
art = " /\\_/\\\n( o.o )"
"##,
        )
        .unwrap();
        assert_eq!(c.mode.as_deref(), Some("plugged-only"));
        assert_eq!(c.inhibit_lid, Some(true));
        assert_eq!(c.splash.as_deref(), Some("coffee"));

        let mut warnings = Vec::new();
        let splashes = build_splashes(&c, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(splashes.len(), builtin_splashes().len() + 2);
        let hello = splashes.iter().find(|s| s.name == "hello").unwrap();
        assert_eq!(hello.color, Color::Rgb(0xff, 0x00, 0xff));
        let cat = splashes.iter().find(|s| s.name == "cat").unwrap();
        match &cat.source {
            SplashSource::Art(lines) => assert_eq!(lines.len(), 2),
            _ => panic!("cat should be art"),
        }
    }

    #[test]
    fn bad_splash_defs_warn_and_skip() {
        let c: FileConfig = toml::from_str(
            r#"
[[splashes]]
name = "broken"
text = ["A"]
art = "also art"

[[splashes]]
name = "badcolor"
text = ["B"]
color = "chartreuse-ish"
"#,
        )
        .unwrap();
        let mut warnings = Vec::new();
        let splashes = build_splashes(&c, &mut warnings);
        // "broken" is skipped, "badcolor" is kept with the default color.
        assert_eq!(splashes.len(), builtin_splashes().len() + 1);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        let bad = splashes.iter().find(|s| s.name == "badcolor").unwrap();
        assert_eq!(bad.color, Color::Green);
    }

    #[test]
    fn state_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let state = State {
            mode: Some("plugged-only".into()),
            inhibit_lid: Some(true),
            splash: Some("coffee".into()),
        };
        save_state_to(&path, &state).unwrap();
        assert_eq!(load_state_from(&path), state);
        // No leftover temp file from the atomic write.
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn bad_state_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        assert_eq!(load_state_from(&path), State::default(), "missing file");
        std::fs::write(&path, "not toml [[[").unwrap();
        assert_eq!(load_state_from(&path), State::default(), "broken file");
        std::fs::write(&path, "mode = \"sometimes\"").unwrap();
        assert_eq!(load_state_from(&path).mode, None, "unknown mode dropped");
    }

    #[test]
    fn base_path_rules() {
        // What counts as absolute is platform-specific: "/xdg" has no drive
        // letter, so Windows considers it relative.
        #[cfg(windows)]
        let (abs, home_dir) = ("C:\\xdg", "C:\\Users\\u");
        #[cfg(not(windows))]
        let (abs, home_dir) = ("/xdg", "/home/u");
        let home = || Some(PathBuf::from(home_dir));

        // Explicit absolute override wins.
        assert_eq!(
            resolve_base(Some(abs.into()), home(), &[".config"]),
            Some(PathBuf::from(abs))
        );
        // A relative override is treated as unset, per the XDG spec.
        assert_eq!(
            resolve_base(Some("rel/ative".into()), home(), &[".config"]),
            Some(PathBuf::from(home_dir).join(".config"))
        );
        // Multi-segment fallback, as used for the state dir.
        assert_eq!(
            resolve_base(None, home(), &[".local", "state"]),
            Some(PathBuf::from(home_dir).join(".local").join("state"))
        );
        // Nothing to go on: None, never the current directory.
        assert_eq!(resolve_base(None, None, &[".config"]), None);
    }

    #[test]
    fn real_paths_end_in_the_app_dir() {
        // Whatever the platform resolves to, the tail must be stable.
        if let Some(p) = config_path() {
            assert!(p.is_absolute(), "{p:?} should be absolute");
            assert!(p.ends_with("sleepless/config.toml"), "{p:?}");
        }
        if let Some(p) = state_path() {
            assert!(p.is_absolute(), "{p:?} should be absolute");
            assert!(p.ends_with("sleepless/state.toml"), "{p:?}");
        }
    }

    #[test]
    fn color_parsing() {
        assert_eq!(parse_color("green"), Some(Color::Green));
        assert_eq!(parse_color("LightCyan"), Some(Color::LightCyan));
        assert_eq!(parse_color("#22c55e"), Some(Color::Rgb(0x22, 0xc5, 0x5e)));
        assert_eq!(parse_color("nope"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#12345g"), None);
    }
}

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
use std::time::{Duration, Instant};

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
    Some(app_file(base?, "state.toml"))
}

/// The layout under whichever base directory the platform gives us.
fn app_file(base: PathBuf, name: &str) -> PathBuf {
    base.join("sleepless").join(name)
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

/// Forget the saved runtime settings (`--reset-state`). A missing file is success:
/// the flag asks to end up with no saved state, not to prove there was some.
pub fn clear_state() -> std::io::Result<()> {
    let Some(path) = state_path() else {
        return Ok(());
    };
    clear_state_at(&path)
}

fn clear_state_at(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// How long to wait before retrying a state write that failed.
const SAVE_RETRY: Duration = Duration::from_secs(30);

/// Writes [`State`] when it changes, and not otherwise.
///
/// The failure path is why this is a type rather than three lines in the loop. The
/// event loop runs four times a second, and leaving the last-written value untouched
/// after an error meant a read-only state directory produced four failed writes a
/// second for as long as the program ran, behind a single warning shown once.
pub struct StateWriter {
    saved: State,
    retry_at: Option<Instant>,
    warned: bool,
}

impl StateWriter {
    /// Seeded with the state at startup, so nothing is written to disk until the
    /// user actually changes something.
    pub fn new(current: State) -> Self {
        Self {
            saved: current,
            retry_at: None,
            warned: false,
        }
    }

    /// Persist `want` if it differs from what was last written successfully.
    /// Returns a message to show the user exactly once, the first time a write fails.
    ///
    /// `write` is a parameter so the failure path is testable without having to
    /// arrange an unwritable directory on every platform CI runs.
    pub fn sync(
        &mut self,
        now: Instant,
        want: &State,
        write: impl FnOnce(&State) -> std::io::Result<()>,
    ) -> Option<String> {
        if *want == self.saved || self.retry_at.is_some_and(|t| now < t) {
            return None;
        }
        match write(want) {
            Ok(()) => {
                self.saved = want.clone();
                self.retry_at = None;
                None
            }
            Err(e) => {
                self.retry_at = Some(now + SAVE_RETRY);
                if std::mem::replace(&mut self.warned, true) {
                    None
                } else {
                    Some(format!("state: settings won't persist: {e}"))
                }
            }
        }
    }
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
    Some(app_file(base?, "config.toml"))
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
            Ok(c) => {
                warn_unknown_keys(&s, &mut warnings);
                c
            }
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

/// Keys this program understands, and the reason it does not use
/// `deny_unknown_fields`: that would make a typo fatal, and a config file must
/// never stop the machine from staying awake. serde's own default is the opposite
/// failure -- a misspelled `inhibit_lid` did exactly nothing and said nothing, in a
/// file whose documented contract is that bad entries warn. So the raw table is
/// diffed against these lists and anything unrecognised is reported and ignored.
pub const KNOWN_TOP: &[&str] = &[
    "mode",
    "inhibit_lid",
    "tray",
    "pulse",
    "why",
    "splash",
    "splashes",
];
pub const KNOWN_SPLASH: &[&str] = &[
    "name",
    "text",
    "art",
    "art_file",
    "color",
    "pulse_color",
    "paused_color",
];

fn warn_unknown_keys(src: &str, warnings: &mut Vec<String>) {
    let Ok(table) = src.parse::<toml::Table>() else {
        return; // already reported by the typed parse
    };
    for k in table.keys() {
        if !KNOWN_TOP.contains(&k.as_str()) {
            warnings.push(format!("config: unknown key {k:?} (ignored)"));
        }
    }
    let Some(splashes) = table.get("splashes").and_then(|v| v.as_array()) else {
        return;
    };
    for (i, def) in splashes.iter().enumerate() {
        let Some(t) = def.as_table() else { continue };
        let name = t
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("splash-{}", i + 1));
        for k in t.keys() {
            if !KNOWN_SPLASH.contains(&k.as_str()) {
                warnings.push(format!(
                    "config: splash {name:?}: unknown key {k:?} (ignored)"
                ));
            }
        }
    }
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
    use std::cell::Cell;

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
        // This used to be two `if let Some(p)` arms, which asserted nothing at all
        // on a host where neither HOME nor XDG_*/APPDATA resolved -- a test that
        // passes by having nothing to check. The layout is now checked against an
        // injected base, unconditionally...
        let base = PathBuf::from(if cfg!(windows) { "C:\\base" } else { "/base" });
        for (name, tail) in [("config.toml", "config.toml"), ("state.toml", "state.toml")] {
            let p = app_file(base.clone(), name);
            assert!(p.ends_with(PathBuf::from("sleepless").join(tail)), "{p:?}");
        }

        // ...and the real functions are then required to resolve. Every machine
        // that can run this test has HOME (or APPDATA/LOCALAPPDATA) set, so "both
        // resolved to nothing" means the resolution is broken, not absent.
        let mut checked = 0;
        for p in [config_path(), state_path()].into_iter().flatten() {
            assert!(p.is_absolute(), "{p:?} should be absolute");
            assert!(
                p.parent().is_some_and(|d| d.ends_with("sleepless")),
                "{p:?}"
            );
            checked += 1;
        }
        assert_eq!(
            checked, 2,
            "config_path()/state_path() resolved to nothing -- set HOME, XDG_*_HOME or %APPDATA%"
        );
    }

    #[test]
    fn clearing_state_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        // Missing file is success: --reset-state asks to end with no saved state.
        clear_state_at(&path).expect("clearing a missing file should succeed");
        save_state_to(&path, &State::default()).unwrap();
        assert!(path.exists());
        clear_state_at(&path).unwrap();
        assert!(!path.exists());
        clear_state_at(&path).expect("clearing twice should succeed");
    }

    #[test]
    fn state_writer_only_writes_on_change() {
        let start = State::default();
        let mut w = StateWriter::new(start.clone());
        let now = Instant::now();
        let writes = Cell::new(0);
        let count = |_: &State| {
            writes.set(writes.get() + 1);
            Ok(())
        };
        assert!(w.sync(now, &start, count).is_none());
        assert_eq!(writes.get(), 0, "the startup state is already on disk");

        let changed = State {
            splash: Some("coffee".into()),
            ..Default::default()
        };
        assert!(w.sync(now, &changed, count).is_none());
        assert_eq!(writes.get(), 1);
        // Same value again: nothing to do.
        assert!(w.sync(now, &changed, count).is_none());
        assert_eq!(writes.get(), 1);
    }

    #[test]
    fn a_failing_state_write_warns_once_and_backs_off() {
        // The regression: the loop runs four times a second and the failed value was
        // never recorded, so an unwritable state dir meant four failed writes a
        // second forever, behind a single warning shown once.
        let mut w = StateWriter::new(State::default());
        let t0 = Instant::now();
        let want = State {
            splash: Some("coffee".into()),
            ..Default::default()
        };
        let attempts = Cell::new(0);
        let fail = |_: &State| {
            attempts.set(attempts.get() + 1);
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        };

        let first = w.sync(t0, &want, fail);
        assert!(
            first.is_some_and(|m| m.contains("won't persist")),
            "the first failure warns"
        );
        assert_eq!(attempts.get(), 1);

        // Every frame for the next 30 s: no second warning, and no syscall either.
        for ms in [250, 500, 29_000] {
            assert!(
                w.sync(t0 + Duration::from_millis(ms), &want, fail)
                    .is_none()
            );
        }
        assert_eq!(
            attempts.get(),
            1,
            "backed off instead of retrying every frame"
        );

        // After the backoff it tries again, and still says nothing further.
        assert!(w.sync(t0 + SAVE_RETRY, &want, fail).is_none());
        assert_eq!(attempts.get(), 2, "retries once the backoff expires");
    }

    #[test]
    fn state_writer_recovers_after_the_directory_becomes_writable() {
        let mut w = StateWriter::new(State::default());
        let t0 = Instant::now();
        let want = State {
            splash: Some("coffee".into()),
            ..Default::default()
        };
        assert!(
            w.sync(t0, &want, |_| Err(std::io::Error::other("nope")))
                .is_some()
        );
        assert!(w.sync(t0 + SAVE_RETRY, &want, |_| Ok(())).is_none());
        // Written now, so an unchanged value must not be written again.
        let writes = Cell::new(0);
        w.sync(t0 + SAVE_RETRY * 2, &want, |_| {
            writes.set(writes.get() + 1);
            Ok(())
        });
        assert_eq!(writes.get(), 0);
    }

    #[test]
    fn unknown_keys_warn_and_are_ignored() {
        // serde ignores what it does not know, so this used to be silent: a typo had
        // no effect and produced no complaint.
        let mut warnings = Vec::new();
        warn_unknown_keys(
            r#"
mode = "always"
inhibitlid = true

[[splashes]]
name = "hello"
text = ["HI"]
colour = "green"
"#,
            &mut warnings,
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings.iter().any(|w| w.contains("\"inhibitlid\"")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("\"hello\"") && w.contains("\"colour\"")),
            "{warnings:?}"
        );
    }

    #[test]
    fn every_known_key_is_actually_accepted() {
        // KNOWN_TOP/KNOWN_SPLASH are a second list of the fields on FileConfig and
        // SplashDef, so they can drift. A config using all of them must parse and
        // must produce no warning at all.
        let src = r#"
mode = "always"
inhibit_lid = false
tray = true
pulse = true
why = "because"
splash = "coffee"

[[splashes]]
name = "hello"
text = ["HI"]
color = "green"
pulse_color = "lightgreen"
paused_color = "darkgray"
"#;
        let cfg: FileConfig = toml::from_str(src).expect("every known key must deserialize");
        let mut warnings = Vec::new();
        warn_unknown_keys(src, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        // `art` and `art_file` are the two alternatives to `text`, checked separately
        // because a splash takes exactly one of the three.
        for alt in ["art = \"x\"", "art_file = \"/tmp/x\""] {
            let src = format!("[[splashes]]\nname = \"a\"\n{alt}\n");
            let mut warnings = Vec::new();
            warn_unknown_keys(&src, &mut warnings);
            assert!(warnings.is_empty(), "{alt}: {warnings:?}");
        }
        assert_eq!(cfg.splashes.len(), 1);
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

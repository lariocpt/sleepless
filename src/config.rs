//! Config file: `$XDG_CONFIG_HOME/nosleep/config.toml` (usually
//! `~/.config/nosleep/config.toml`).
//!
//! Everything is optional and CLI flags win over the file. Broken entries are
//! skipped with a warning shown in the TUI, never fatal — a bad config must
//! not stop the machine from staying awake.

use crate::art::{Splash, SplashSource};
use ratatui::style::Color;
use serde::Deserialize;
use std::path::PathBuf;

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

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("nosleep").join("config.toml")
}

pub fn load() -> Loaded {
    let path = config_path();
    let mut warnings = Vec::new();
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
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(p)
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
    fn color_parsing() {
        assert_eq!(parse_color("green"), Some(Color::Green));
        assert_eq!(parse_color("LightCyan"), Some(Color::LightCyan));
        assert_eq!(parse_color("#22c55e"), Some(Color::Rgb(0x22, 0xc5, 0x5e)));
        assert_eq!(parse_color("nope"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#12345g"), None);
    }
}

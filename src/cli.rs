//! Command line, and the precedence rules that turn it plus the config file plus
//! the saved state into one set of runtime settings.
//!
//! The precedence itself used to live inline in `main`, which meant the one piece
//! of logic every launch depends on — and the one users notice when it is wrong —
//! was the only piece with no test in front of it.

use crate::app::Mode;
use crate::art::Splash;
use crate::config::{self, FileConfig, State};
use clap::Parser;

/// Keep your computer awake while this runs. Close the terminal to stop.
///
/// Options not given on the command line fall back to
/// ~/.config/sleepless/config.toml, then to built-in defaults.
#[derive(Parser, Debug, Default)]
#[command(name = "sleepless", version, about)]
pub struct Args {
    /// Only keep awake while on AC power (toggle at runtime with 'm')
    #[arg(long)]
    pub plugged_only: bool,

    /// Keep awake regardless of power source, overriding any saved setting
    #[arg(long, conflicts_with = "plugged_only")]
    pub always: bool,

    /// Also block lid-close suspend (Linux only: logind handle-lid-switch)
    #[arg(long)]
    pub inhibit_lid: bool,

    /// Don't show a system tray icon (Linux only)
    #[arg(long)]
    pub no_tray: bool,

    /// Disable the pulse animation
    #[arg(long)]
    pub no_pulse: bool,

    /// Forget the saved runtime settings and start from config.toml defaults
    #[arg(long)]
    pub reset_state: bool,

    /// Start on this splash screen (by name); cycle with Left/Right
    #[arg(long)]
    pub splash: Option<String>,

    /// Reason recorded with the lock, e.g. in `systemd-inhibit --list`
    /// [default: "I can't get no sleep"]
    #[arg(long)]
    pub why: Option<String>,

    /// Run headless for N seconds, print status, then exit
    #[arg(long, value_name = "SECS", hide = true)]
    pub smoke: Option<u64>,
}

/// Everything the app needs to start, after all three sources have been merged.
pub struct Settings {
    pub mode: Mode,
    pub lid: bool,
    pub pulse: bool,
    pub tray: bool,
    pub why: String,
    pub splashes: Vec<Splash>,
    pub splash_idx: usize,
    pub warnings: Vec<String>,
}

pub const DEFAULT_WHY: &str = "I can't get no sleep";

/// Merge the three sources. Precedence is **flags > saved state > config.toml**,
/// and it is documented that way in the README, so it is asserted here rather than
/// left to be re-derived from the `or_else` chains below.
///
/// `warnings` comes in pre-populated with whatever loading the config produced, and
/// comes out with anything this merge noticed added to it.
pub fn resolve(
    args: &Args,
    file: &FileConfig,
    state: &State,
    mut warnings: Vec<String>,
) -> Settings {
    // `--always` is the only way back out of a saved `plugged-only`, which is why
    // it exists: state outranks the config file, so editing config.toml would not
    // do it.
    let mode = if !args.always
        && (args.plugged_only
            || state.mode.as_deref().or(file.mode.as_deref()) == Some(Mode::PluggedOnly.label()))
    {
        Mode::PluggedOnly
    } else {
        Mode::Always
    };

    let lid = args.inhibit_lid || state.inhibit_lid.or(file.inhibit_lid).unwrap_or(false);
    let pulse = !args.no_pulse && file.pulse.unwrap_or(true);
    let tray = !args.no_tray && file.tray.unwrap_or(true);
    let why = args
        .why
        .clone()
        .or_else(|| file.why.clone())
        .unwrap_or_else(|| DEFAULT_WHY.into());

    let splashes = config::build_splashes(file, &mut warnings);
    let splash_arg = args
        .splash
        .as_deref()
        .or(state.splash.as_deref())
        .or(file.splash.as_deref());
    let splash_idx = match splash_arg {
        None => 0,
        Some(name) => splashes
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| {
                warnings.push(format!("splash {name:?} not found, using the first one"));
                0
            }),
    };

    Settings {
        mode,
        lid,
        pulse,
        tray,
        why,
        splashes,
        splash_idx,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn file(toml: &str) -> FileConfig {
        toml::from_str(toml).unwrap()
    }
    fn resolved(args: Args, f: &str, s: State) -> Settings {
        resolve(&args, &file(f), &s, Vec::new())
    }
    fn state(mode: &str) -> State {
        State {
            mode: Some(mode.into()),
            ..Default::default()
        }
    }

    #[test]
    fn clap_definition_is_valid() {
        // Catches conflicting flags, duplicate long names and bad defaults at test
        // time rather than on a user's first run.
        Args::command().debug_assert();
    }

    #[test]
    fn precedence_is_flags_then_state_then_file() {
        // config.toml alone decides on a first run.
        let s = resolved(Args::default(), "mode = \"plugged-only\"", State::default());
        assert_eq!(s.mode, Mode::PluggedOnly, "config.toml should apply");

        // Saved state outranks the file.
        let s = resolved(Args::default(), "mode = \"plugged-only\"", state("always"));
        assert_eq!(s.mode, Mode::Always, "state should outrank config.toml");

        // And a flag outranks both. This is what --always is for.
        let args = Args {
            always: true,
            ..Default::default()
        };
        let s = resolved(args, "mode = \"plugged-only\"", state("plugged-only"));
        assert_eq!(s.mode, Mode::Always, "--always should outrank state");

        let args = Args {
            plugged_only: true,
            ..Default::default()
        };
        let s = resolved(args, "mode = \"always\"", state("always"));
        assert_eq!(
            s.mode,
            Mode::PluggedOnly,
            "--plugged-only should outrank state"
        );
    }

    #[test]
    fn mode_label_is_the_spelling_config_and_state_use() {
        // resolve() compares against Mode::PluggedOnly.label(); if that ever stops
        // matching the string the docs and the state file use, every saved
        // plugged-only setting silently becomes "always".
        assert_eq!(Mode::PluggedOnly.label(), "plugged-only");
        assert_eq!(Mode::Always.label(), "always");
    }

    #[test]
    fn lid_and_toggles_follow_the_same_order() {
        let s = resolved(Args::default(), "inhibit_lid = true", State::default());
        assert!(s.lid, "config.toml should apply");

        let st = State {
            inhibit_lid: Some(false),
            ..Default::default()
        };
        let s = resolved(Args::default(), "inhibit_lid = true", st);
        assert!(!s.lid, "state should outrank config.toml");

        let args = Args {
            inhibit_lid: true,
            ..Default::default()
        };
        let st = State {
            inhibit_lid: Some(false),
            ..Default::default()
        };
        let s = resolved(args, "", st);
        assert!(s.lid, "--inhibit-lid should outrank state");
    }

    #[test]
    fn why_falls_back_through_flag_file_default() {
        let s = resolved(Args::default(), "", State::default());
        assert_eq!(s.why, DEFAULT_WHY);
        let s = resolved(Args::default(), "why = \"rendering\"", State::default());
        assert_eq!(s.why, "rendering");
        let args = Args {
            why: Some("compiling".into()),
            ..Default::default()
        };
        let s = resolved(args, "why = \"rendering\"", State::default());
        assert_eq!(s.why, "compiling");
    }

    #[test]
    fn tray_and_pulse_default_on_and_are_switched_off_by_flags_or_file() {
        let s = resolved(Args::default(), "", State::default());
        assert!(s.tray && s.pulse, "both default on");
        let s = resolved(
            Args::default(),
            "tray = false\npulse = false",
            State::default(),
        );
        assert!(!s.tray && !s.pulse, "config.toml can switch them off");
        let args = Args {
            no_tray: true,
            no_pulse: true,
            ..Default::default()
        };
        let s = resolved(args, "", State::default());
        assert!(!s.tray && !s.pulse, "flags can switch them off");
    }

    #[test]
    fn splash_selection_and_the_unknown_name_warning() {
        let s = resolved(Args::default(), "", State::default());
        assert_eq!(s.splash_idx, 0);

        let s = resolved(Args::default(), "splash = \"coffee\"", State::default());
        assert_eq!(s.splashes[s.splash_idx].name, "coffee");

        // State outranks the file here too.
        let st = State {
            splash: Some("brooklyn".into()),
            ..Default::default()
        };
        let s = resolved(Args::default(), "splash = \"coffee\"", st);
        assert_eq!(s.splashes[s.splash_idx].name, "brooklyn");

        // An unknown name is a warning and the first splash, never a failure to
        // start: the machine staying awake matters more than the picture.
        let args = Args {
            splash: Some("nope".into()),
            ..Default::default()
        };
        let s = resolved(args, "", State::default());
        assert_eq!(s.splash_idx, 0);
        assert!(
            s.warnings.iter().any(|w| w.contains("nope")),
            "{:?}",
            s.warnings
        );
    }
}

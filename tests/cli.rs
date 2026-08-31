//! The binary, run the way a user runs it.
//!
//! Everything here goes through a throwaway `HOME`: the suite must never read or
//! write the real `~/.config/sleepless` or `~/.local/state/sleepless`, and a test
//! that quietly depends on the developer's own config is a test that passes for the
//! wrong reason on one machine and fails on CI.

use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sleepless"))
}

/// A sleepless invocation with every "where does this program keep things"
/// variable pointed at a fresh directory.
fn sandboxed(home: &Path) -> Command {
    let mut c = bin();
    c.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        // The Windows equivalents, so the sandbox holds there too.
        .env("USERPROFILE", home)
        .env("APPDATA", home.join("config"))
        .env("LOCALAPPDATA", home.join("state"));
    c
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn is_printable_ascii(s: &str) -> bool {
    s.chars()
        .all(|c| c == '\n' || c == ' ' || c.is_ascii_graphic())
}

#[test]
fn version_matches_the_package() {
    // A tag that ships a binary claiming a different version is the failure the
    // release workflow's version gate exists for; this is the same check, locally.
    let out = bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        stdout(&out).trim(),
        format!("sleepless {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_works_when_it_is_not_a_terminal() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success(), "--help must work when piped");
    let text = stdout(&out);
    for flag in ["--plugged-only", "--always", "--reset-state", "--why"] {
        assert!(text.contains(flag), "--help omits {flag}");
    }
    assert!(
        !text.contains("--smoke"),
        "--smoke is a test hook, hidden on purpose"
    );
}

#[test]
fn every_documented_flag_is_accepted() {
    let home = TempDir::new().unwrap();
    for args in [
        vec!["--always"],
        vec!["--plugged-only"],
        vec!["--inhibit-lid"],
        vec!["--no-tray"],
        vec!["--no-pulse"],
        vec!["--reset-state"],
        vec!["--splash", "coffee"],
        vec!["--why", "testing"],
        vec!["--always", "--inhibit-lid", "--no-tray", "--no-pulse"],
    ] {
        let out = sandboxed(home.path())
            .args(&args)
            .args(["--smoke", "0"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?} was rejected: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout(&out).starts_with("sleepless - "), "{args:?}");
    }
}

#[test]
fn contradictory_modes_are_refused_rather_than_guessed() {
    let home = TempDir::new().unwrap();
    let out = sandboxed(home.path())
        .args(["--always", "--plugged-only", "--smoke", "0"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "--always --plugged-only must not be accepted"
    );
}

#[test]
fn smoke_output_is_ascii_on_the_error_path_too() {
    // CI used to assert this on the run where everything worked, which is the one
    // path that cannot produce a non-ASCII byte. The interesting output -- D-Bus
    // error strings, config paths, the --why text -- only appears when something
    // has gone wrong.
    let home = TempDir::new().unwrap();
    let cfg = home.path().join("config").join("sleepless");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "mode = \"sometimes\"\nnaptime = true\nwhy = \"caf\u{e9} \u{2014} \u{1f50b}\"\n",
    )
    .unwrap();

    let mut cmd = sandboxed(home.path());
    #[cfg(target_os = "linux")]
    cmd.env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent");
    let out = cmd.args(["--always", "--smoke", "0"]).output().unwrap();

    assert!(
        out.status.success(),
        "a broken config must never stop it running"
    );
    let text = stdout(&out);
    assert!(
        is_printable_ascii(&text),
        "non-ASCII reached the scriptable output:\n{text}"
    );
    assert!(
        text.contains("unknown mode"),
        "the bad mode should be reported:\n{text}"
    );
    assert!(
        text.contains("naptime"),
        "an ignored key should be reported, not silently dropped:\n{text}"
    );
    #[cfg(target_os = "linux")]
    assert!(
        text.contains("NOT INHIBITING"),
        "with no bus at all:\n{text}"
    );
}

#[test]
fn reset_state_forgets_the_saved_settings() {
    let home = TempDir::new().unwrap();
    let state = home
        .path()
        .join("state")
        .join("sleepless")
        .join("state.toml");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    std::fs::write(&state, "mode = \"plugged-only\"\nsplash = \"coffee\"\n").unwrap();

    // Without the flag the saved mode wins, which is the behaviour --reset-state
    // exists to undo.
    let out = sandboxed(home.path())
        .args(["--smoke", "0"])
        .output()
        .unwrap();
    assert!(stdout(&out).contains("plugged-only"), "{}", stdout(&out));
    assert!(state.exists());

    let out = sandboxed(home.path())
        .args(["--reset-state", "--smoke", "0"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        !state.exists(),
        "--reset-state should remove the state file"
    );
    // And it is fine to ask twice.
    let out = sandboxed(home.path())
        .args(["--reset-state", "--smoke", "0"])
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn the_sandbox_really_is_one() {
    // If this ever fails, every other test here has been reading the developer's
    // own config and proving nothing.
    let home = TempDir::new().unwrap();
    let out = sandboxed(home.path())
        .args(["--always", "--smoke", "0"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        !home.path().join("config").join("sleepless").exists(),
        "a run that changes nothing must not create a config"
    );
}

# Changelog

All notable changes to sleepless are recorded here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The `## [X.Y.Z]` heading shape is load-bearing, not a style choice:
`.github/workflows/release.yml` extracts everything between one such heading and the
next to use as the GitHub Release notes, and refuses to publish a tag that has no
section here. The `version` in `Cargo.toml` must match the tag being released.

## [Unreleased]

## [0.1.1] - 2026-08-31

A review release: everything below came out of a full pass over 0.1.0 the week after
it shipped. Nothing here changes what the tool does to a machine that was already
working; all of it is about the cases where it said one thing and did another.

### Fixed

- **The LAN apps-plane channel had never published anything.** Its Jenkins pipeline
  opened `pipeline {` twice and never closed the first `options` block, so `groovyc`
  rejected the file and every build failed at parse time — since the day it was
  written, on a two-minute poll. The plane was still advertising the pre-rename
  `nosleep 0.1.0+de316f8`, `install.sh -s -- sleepless` resolved nothing, and every
  machine provisioned since silently fell back to compiling from a vendored snapshot
  while believing it had fetched a verified artifact. The job now mirrors the public
  release the way color-terminal's and smartopen's do, so the LAN serves the same
  bytes as every other channel and asserts it.
- **A lost lock kept reporting itself held.** `active()` was `guard.is_some()`, and
  nothing ever re-checked. If the compositor or logind restarted, the cookie named a
  session that no longer existed and the pipe fd had nobody on the other end — but
  the connection stayed up, the fd stayed valid, the banner stayed green, and the
  retry path never ran because it only fires when there is no guard at all. The
  owning bus name is recorded when a lock is taken and re-checked on the same
  two-second tick that polls the power source; a lock whose owner has gone is dropped
  and re-acquired.
- **Re-running the release workflow deleted the live release.**
  `create-gh-release-action` deletes an existing release before recreating it, and
  because the archives were not reproducible it gave back different bytes — silently
  invalidating the sha256 values pinned in the Homebrew formula and the AUR PKGBUILD,
  so `brew install` and `makepkg` would have started accusing users of tampering.
- **The release was published before it was built.** The workflow created a public,
  non-draft release as its first step and let uploads trail in over the next three
  minutes; it ran no tests at all, because `ci.yml` does not fire on tags. 0.1.0's own
  history has two red runs against a release that stayed up regardless. The release is
  now created as a draft, its own published assets are downloaded back and verified,
  the tag is rebuilt from scratch and required to produce byte-identical archives, and
  only then is it flipped live. A failure afterwards re-drafts it, so `latest` falls
  back to the previous good release. Re-publishing an older tag can no longer demote
  the current one.
- **The archives are reproducible.** `tar acf` recorded the build time, the building
  user and the machine's umask. Packaging is now normalised (sorted members, fixed
  mtimes, `0:0` ownership, fixed modes, `gzip -n`) and the compiler's embedded paths
  are remapped, so `tools/package.py --target ...` on any machine with the toolchain
  in `.rust-version` gives back the published checksum. The release workflow asserts
  that on all eight targets rather than claiming it.
- **Nothing checked that the tag matched the version.** Tagging `v0.1.2` without
  bumping `Cargo.toml` would have shipped a binary reporting `0.1.0`. The release now
  refuses to start if the two disagree, or if `CHANGELOG.md` has no section for it —
  0.1.0 shipped with an entirely empty release body.
- **The website contradicted the program.** It argued the product's whole case with
  "there is no `--stop` flag, no PID file and no state directory to get out of sync",
  while the README documented a state file in detail. Its block font claimed to be
  "lifted verbatim from `src/art.rs`" and was missing every digit and every
  punctuation glyph. It quoted a 750 ms pulse, which is the tray's cadence, not the
  splash's 500 ms. It offered two install methods while the README offered four. All
  of that is now generated or asserted against the code by `tests/docs.rs`, which
  checks every documented flag, config key, keybinding, built-in splash, font
  character, cadence and install command.
- **CI could not fail in three places.** `cancel-in-progress` was on for main, so two
  quick merges left the first commit with no completed gate; the MSRV job never ran on
  Windows although the 1.89 floor comes from a Windows-and-macOS dependency; and arm64
  ran `cargo test` and nothing else while the website claimed CI verified the lock
  there. The smoke probes moved into `test/smoke.sh` and `test/smoke.ps1`, real files
  that shellcheck sees and anyone can run, and every runner now executes one. Linux
  gained a probe it never had: it asks logind whether the inhibitor is listed, then
  `kill -9`s the process and asserts it is gone.
- **The ASCII guarantee only covered the path that could not break it.** CI asserted
  the `--smoke` output was ASCII on the run where everything worked; the D-Bus error
  strings, config paths and `--why` text that could contain anything else only appear
  when something has gone wrong. The status line is now forced to printable ASCII at
  the point it is printed.
- **The website deployed with nothing in front of it.** `pages.yml` published any
  docs push while CI went red afterwards, if it ran at all. It now runs the site's own
  checks first: no external request except Google Fonts (checked by walking every tag
  and attribute, not by a grep that misses single quotes, `srcset`, `url()` and
  `<iframe>`), every local asset present, the generated font up to date, and the
  docs-versus-code suite.
- A failing state write retried four times a second forever behind a single warning;
  it now backs off. A misspelled config key was silently ignored in a file whose
  documented contract is that bad entries warn. `art_file` art wider than 65533
  columns panicked in a debug build and wrapped in release, so a 70 000-column line
  "fitted" an 80-column terminal. And a path test passed by asserting nothing when
  neither `HOME` nor `XDG_*` was set.

### Added

- `--reset-state`, to forget the remembered settings without hunting for the file.
- `CHANGELOG.md`, which is where release notes now come from.
- `tests/docs.rs` and `tests/cli.rs`: the documentation gate, and the binary driven
  the way a user drives it, inside a sandboxed `HOME` so the suite can never read or
  write real settings.
- `tools/`: `package.py` (reproducible archives), `check-site.py` (the website gate),
  `gen-site-font.py` (the hero's font, generated from `src/art.rs`), and
  `check-workflows.py`, which asserts the workflow properties above so none of them
  can quietly come back.
- Build provenance attestation on every archive, using the permissions the workflow
  had been granting itself and never using.
- A crates.io check after publishing, because that channel sits outside the workflow
  and 0.1.0 reached it seven hours before the release existed.
- The README documents the AUR package, how to verify a download, and how to
  reproduce one.

### Changed

- The toolchain is pinned in `.rust-version` and used by every CI job. `stable` meant
  a rustfmt or clippy release could turn a green branch red with no commit, and a
  release archive only reproduces under the compiler that built it.
- Every Linux target is built natively; `cross` is gone. It built `x86_64-musl` in a
  container whose paths differ from a host build's, so the two could never produce the
  same bytes — and Rust ships self-contained musl CRT objects, which is why the
  aarch64 build had already stopped using it.
- The crate is a library plus a thin binary, so the flag-precedence rules and the
  inhibition backend can be tested rather than only compiled.

## [0.1.0] - 2026-08-25

First public release. A terminal keep-awake tool whose every lock is bound to the
process: closing the terminal, or `kill -9`, restores normal sleep with nothing left
to clean up.

### Added

- **Two independent Linux locks, neither needing a clean shutdown.**
  `org.freedesktop.ScreenSaver.Inhibit` returns a cookie scoped to the D-Bus
  connection, and `org.freedesktop.login1.Manager.Inhibit("sleep:idle", …, "block")`
  hands back a pipe fd the kernel closes when the process dies. Either may be missing;
  with neither, it still starts and says so rather than refusing to run.
- **macOS and Windows**, through `IOPMAssertionCreateWithName` and
  `SetThreadExecutionState` — both process-scoped, so the core guarantee holds.
- **A battery rule.** `--plugged-only` releases every lock the moment you unplug and
  takes them back when AC returns, checked every two seconds.
- **A tray icon** on Linux (StatusNotifierItem), which re-registers itself when the
  bar restarts.
- **Opt-in lid-close blocking**, degrading with a visible note when polkit refuses it.
- **Custom splash screens** in `config.toml`: block-font text, inline ASCII art, or an
  art file, each with its own colours.
- **Settings persistence** for the mode, lid and splash toggles, in
  `$XDG_STATE_HOME/sleepless/state.toml`.
- **`--smoke N`**, a headless mode that takes the locks, prints plain-ASCII status and
  exits, with no TTY.
- Prebuilt binaries for eight targets, a Homebrew tap, an AUR package, and a website.

[Unreleased]: https://github.com/lariocpt/sleepless/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/lariocpt/sleepless/releases/tag/v0.1.1
[0.1.0]: https://github.com/lariocpt/sleepless/releases/tag/v0.1.0

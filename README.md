# sleepless

> **I CAN'T GET NO SLEEP**

Keeps your computer awake for exactly as long as it's running.

![sleepless in action](https://raw.githubusercontent.com/lariocpt/sleepless/main/docs/demo.gif)

Every lock it takes is bound to the process itself, so closing the terminal — or
`kill -9`-ing it, however rudely — restores normal sleep instantly. There is no daemon,
no setting to put back, and nothing left behind to clean up.

```sh
cargo install sleepless
```

[**Website**](https://lariocpt.github.io/sleepless/) — the source is
[`docs/index.html`](docs/index.html), one self-contained file with no build step.

## Why not just use something else

The usual tools change a *setting*: a power profile, a disabled timer, a background
daemon holding a lock on your behalf. All of that outlives the program that set it, and
all of it outlives that program crashing. sleepless has no off switch to forget, because
the process **is** the switch.

|                                  | releases cleanly on `kill -9` | needs a daemon | shows what it holds |
| -------------------------------- | ----------------------------- | -------------- | ------------------- |
| `sleepless`                        | yes                           | no             | yes                 |
| `systemd-inhibit sleep infinity` | yes                           | no             | no                  |
| GUI caffeine-style applets       | usually leaves state behind   | yes            | no                  |

`systemd-inhibit` is the honest closest comparison and it is a fine tool. sleepless is
that, with a face, a battery rule, a tray icon, and a status line that tells you when it
*failed* to get a lock — which is the thing you actually want to know.

## Platform support

The core guarantee holds on all three platforms: every mechanism below is process-scoped,
so nothing survives the process. What differs is how much is exposed.

|                        | Linux                              | macOS                            | Windows                        |
| ---------------------- | ---------------------------------- | -------------------------------- | ------------------------------ |
| Mechanism              | D-Bus ScreenSaver + logind pipe fd | `IOPMAssertionCreateWithName`    | `SetThreadExecutionState`      |
| Dies with the process  | yes — kernel closes the fd         | yes — powerd reaps the assertion | yes — state is per-thread      |
| Keeps the display on   | yes                                | yes                              | yes                            |
| Tray icon              | yes (StatusNotifierItem)           | no                               | no                             |
| Lid-close blocking     | opt-in                             | no                               | no                             |
| Verified by            | daily use + CI                     | CI (`pmset -g assertions`)       | CI (`powercfg /requests`)      |

CI builds and lints all eight targets (linux gnu/musl and macOS/Windows, x86-64 and
arm64) on every push, and on each of the three OSes it starts sleepless, asks the platform
whether the lock is really registered, `kill -9`s it and asserts the lock is gone.

Linux is the developed path and the one that gets used every day. macOS and Windows pass
that automated check on every commit but see far less human use — reports welcome.

**Not supported:** FreeBSD and other Unixes build, but there is no inhibition backend, so
sleepless says so and holds nothing.

## How it works

On Linux, two independent locks, neither of which needs sleepless to shut down politely:

- **`org.freedesktop.ScreenSaver.Inhibit`** (session bus) returns a cookie scoped to the
  D-Bus *connection*. The process dying drops the connection, and the compositor releases
  the inhibit on its own. Implemented by GNOME, KDE, niri and most others.
- **`org.freedesktop.login1.Manager.Inhibit("sleep:idle", …, "block")`** (system bus)
  hands back an open pipe file descriptor, and the lock lasts exactly as long as that fd
  stays open. Closing it isn't something sleepless has to remember — the kernel closes every
  fd a process owns when it dies, `SIGKILL` included.

Either one may be missing and sleepless keeps whatever it can get, showing the rest as
failed. If neither bus exists at all — a container, a bare TTY — it still starts and shows
a red `✗ NOT INHIBITING` banner rather than refusing to run.

Check it from another terminal while it runs:

```sh
systemd-inhibit --list | grep sleepless
```

## Install

```sh
cargo install sleepless                      # crates.io
brew install lariocpt/sleepless/sleepless    # macOS / Linuxbrew
cargo install --path .                       # from a clone
```

Or grab a prebuilt binary from [Releases](https://github.com/lariocpt/sleepless/releases)
— Linux builds are static musl, so they have no glibc floor and run anywhere.

Building from source requires Rust 1.89 or newer. The dependency tree is pure Rust — no C toolchain, no
`pkg-config`, no system libraries — so cross-compiling and static musl builds work without
any extra setup.

## Usage

```sh
sleepless                    # keep the machine awake while running
sleepless --plugged-only     # start in plugged-only mode
sleepless --always           # force always mode, overriding saved settings
sleepless --inhibit-lid      # also block lid-close suspend (Linux)
sleepless --no-tray          # no tray icon (Linux)
sleepless --no-pulse         # no pulse animations
sleepless --splash coffee    # start on a specific splash screen
sleepless --why "compiling all of chromium"
```

Keys: **m** toggle mode · **l** toggle lid block (Linux) · **←/→** cycle splash screens ·
**q** / Esc / Ctrl-C quit. Or just close the terminal.

Colour carries the state: green and pulsing means the locks are held, grey with a yellow
banner means it paused on purpose, red means it wanted the locks and couldn't get them.

### Modes

**always** (default) holds the locks regardless of power source. **plugged-only** releases
them the moment you unplug and takes them back when AC returns, checked every two seconds,
showing `◌ PAUSED — ON BATTERY` in between. Toggle live with **m** or from the tray.

## Configuration

Optional. Linux and macOS use `~/.config/sleepless/config.toml` (respecting
`$XDG_CONFIG_HOME`); Windows uses `%APPDATA%\sleepless\config.toml`. Command-line flags
always win over the file, and a broken config warns inside the TUI and is otherwise
ignored — it can never stop the app from working.

```toml
mode = "plugged-only"     # "always" or "plugged-only"
inhibit_lid = false
tray = true
pulse = true
why = "I can't get no sleep"
splash = "coffee"

# Extra splash screens (cycle with ←/→). Three are built in:
# "default" (I CAN'T GET NO SLEEP), "brooklyn", "coffee".
[[splashes]]
name = "hello"
text = ["HELLO!", "WORLD"]      # rendered with the block font: A-Z 0-9 '-!?.
color = "#ff00ff"               # active colour: name or #rrggbb
pulse_color = "lightmagenta"
paused_color = "darkgray"

[[splashes]]
name = "cat"
art = '''
 /\_/\
( o.o )
 > ^ <
'''                             # verbatim ASCII art…

[[splashes]]
name = "dragon"
art_file = "~/.config/sleepless/dragon.txt"   # …or loaded from a file
color = "lightred"
```

Each splash takes exactly one of `text`, `art`, or `art_file`. Text splashes scale down
automatically on narrow terminals; art that doesn't fit falls back to a one-line banner.

### Settings persistence

Runtime changes — the mode toggle (**m** / tray), the lid-block toggle (**l**), and the
selected splash (**←/→**) — are saved as they happen to `~/.local/state/sleepless/state.toml`
(respecting `$XDG_STATE_HOME`), or `%LOCALAPPDATA%\sleepless\state.toml` on Windows, and
restored next launch.

Precedence is **command-line flags > saved state > config.toml**. So `mode` and friends in
config.toml act as first-run defaults; after you toggle something at runtime the saved
value wins, and a flag like `--always` overrides both for that run. Delete the state file
to go back to your config.toml defaults. A broken or unwritable state file is never fatal.

## Good to know

- **Lid close still suspends by default.** logind treats the lid as a separate channel
  (`handle-lid-switch`) that ignores normal sleep inhibitors — that's why lid-blocking is
  an explicit opt-in. On some systems polkit refuses it; sleepless then keeps the other
  locks and marks lid as `✗ (refused)`.
- **Deliberate suspends still win.** `systemctl suspend -i` and a long power-key press
  bypass block-mode inhibitors. That's by design — yours, and systemd's.
- **sway and Hyprland:** suspend is blocked, but screen blanking isn't. The Wayland
  idle-inhibit protocol needs a surface to attach to and a terminal program has none.
- **Windows on battery:** on Modern Standby laptops, Windows terminates power requests
  some minutes after the sleep timeout when on DC power. `--plugged-only` is the
  dependable mode there.
- **No tray host** (bare TTY, minimal WM)? sleepless runs fine without it and notes
  `tray: unavailable` in the footer. If the bar restarts, the icon re-registers itself.

## Development

- `cargo run -- --smoke 5` — headless: acquire locks, print plain-ASCII status, hold 5 s,
  exit. No TTY needed; this is what CI asserts against.
- `cargo test` — unit tests (sysfs parsing, mode decision table, path rules, art layout).
- `cargo clippy --all-targets -- -D warnings` — must be clean, on **every** target:

  ```sh
  for t in aarch64-apple-darwin x86_64-pc-windows-msvc x86_64-unknown-linux-musl; do
    cargo clippy --target "$t" --all-targets -- -D warnings
  done
  ```

  `cargo check`/`clippy` don't link, so foreign targets need only `rustup target add`.

### Contributing

Two things that aren't obvious from the code:

1. **The process-lifetime-bound rule is absolute.** Only mechanisms that the OS releases
   on process death are allowed — D-Bus connection-scoped cookies, logind pipe fds,
   IOPMAssertions, thread execution state. Anything that needs cleanup after the process
   dies breaks the one guarantee this program makes, and will be declined however
   convenient it is.
2. Clippy and tests must pass on all targets, and the README is updated in the same change.

## License

MIT — see [LICENSE](LICENSE).

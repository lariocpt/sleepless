# nosleep

```
  ████████    ██████    ██      ██  ██  ██████████
    ██      ██          ██████    ██  ██      ██
    ██      ██          ██  ██  ██  ██          ██   …
```
> **I CAN'T GET NO SLEEP**

A terminal app that keeps your computer awake for exactly as long as it's running.
Close the terminal (or kill the process, however rudely) and normal sleep behavior
returns instantly — there is no daemon and nothing to clean up.

- Full-screen TUI with a big block-letter splash, color-coded by state:
  **green & pulsing = holding locks**, gray + yellow banner = paused on battery,
  red banner = wanted locks but couldn't get them.
- Two modes, toggleable live: **always** (default) or **only while plugged in**
  (`--plugged-only`) — on battery it releases the locks and shows
  `◌ PAUSED — ON BATTERY` until AC returns (checked every 2 s).
- A bright system-tray eye (StatusNotifierItem): green open eye while awake
  (gently pulsing), amber closed eye while paused. Left-click toggles the mode;
  the menu has mode radio buttons, a lid-block checkbox, and quit.

## How it works

While active, nosleep holds two process-lifetime-bound locks:

- **`org.freedesktop.ScreenSaver.Inhibit`** (session bus) — on a niri/noctalia setup
  niri implements this and suppresses the idle chain (lock / screen-off / idle
  suspend). Auto-released when the D-Bus connection dies. Also understood by KDE,
  GNOME, and other compositors.
- **`org.freedesktop.login1.Manager.Inhibit("sleep:idle", …, "block")`** (system bus) —
  logind hands back a pipe file descriptor; the lock lasts exactly as long as the fd
  is open. The kernel closes it on process death, even SIGKILL.

Because both locks die with the process, closing the terminal *is* the off switch.

## Building & Running

### Run directly
```sh
cargo run                     # compile and run with default settings
cargo run -- --plugged-only   # pass flags after --
```

### Build binaries
```sh
cargo build           # debug build in target/debug/nosleep
cargo build --release # optimized release build in target/release/nosleep
```

### Install locally
```sh
cargo install --path . # compiles and installs to ~/.cargo/bin/nosleep
```

## Usage

```sh
nosleep                    # keep the machine awake while running
nosleep --plugged-only     # start in plugged-only mode
nosleep --always           # force always mode, overriding saved settings
nosleep --inhibit-lid      # additionally block lid-close suspend
nosleep --no-tray          # no tray icon
nosleep --no-pulse         # no pulse animations
nosleep --splash coffee    # start on a specific splash screen
nosleep --why "compiling all of chromium"   # reason shown in systemd-inhibit --list
```

Keys: **m** toggle mode · **l** toggle lid-close block · **←/→** cycle splash
screens · **q** / Esc / Ctrl-C quit.

Check what it's holding from another terminal:

```sh
systemd-inhibit --list | grep nosleep
```

## Configuration

Optional, at `~/.config/nosleep/config.toml` (respects `$XDG_CONFIG_HOME`).
Command-line flags always win over the file; a broken config warns inside the
TUI and is otherwise ignored — it can never stop the app from working.

```toml
mode = "plugged-only"     # default mode: "always" or "plugged-only"
inhibit_lid = false
tray = true
pulse = true
why = "I can't get no sleep"
splash = "coffee"         # splash to show at startup, by name

# Extra splash screens (cycle with ←/→). Three are built in:
# "default" (I CAN'T GET NO SLEEP), "brooklyn", "coffee".
[[splashes]]
name = "hello"
text = ["HELLO!", "WORLD"]      # rendered with the block font: A-Z 0-9 '-!?.
color = "#ff00ff"               # active color: name or #rrggbb
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
art_file = "~/.config/nosleep/dragon.txt"   # …or loaded from a file
color = "lightred"
```

Each splash takes exactly one of `text`, `art`, or `art_file`. Text splashes
scale down automatically on narrow terminals; art that doesn't fit falls back
to a one-line banner.

### Settings persistence

Runtime changes — the mode toggle (**m** / tray), the lid-block toggle (**l**),
and the selected splash (**←/→**) — are saved as they happen to
`~/.local/state/nosleep/state.toml` (respects `$XDG_STATE_HOME`) and restored on
the next launch. Precedence is: command-line flags > saved state > config.toml.
So `mode` and friends in config.toml act as first-run defaults; after you toggle
something at runtime the saved value wins, and a flag like `--always` or
`--plugged-only` overrides both for that run. Delete the state file to go back
to your config.toml defaults. A broken or unwritable state file is never fatal.

## Good to know

- **Lid close still suspends by default.** logind treats the lid as a separate
  channel (`handle-lid-switch`) that ignores normal sleep inhibitors — that's why
  lid-blocking is an explicit opt-in (`--inhibit-lid` or the `l` key / tray checkbox).
  On some systems polkit refuses it; nosleep then keeps the other locks and marks
  lid as `✗ (refused)`.
- Deliberate suspends still win: `systemctl suspend -i` and a long power-key press
  bypass block-mode inhibitors. That's by design (yours, and systemd's).
- No tray host (bare TTY, minimal WM)? nosleep runs fine without it and notes
  `tray: unavailable` in the footer. If the bar restarts, the icon re-registers
  by itself.
- Non-Linux (macOS/Windows) builds fall back to the `keepawake` crate — untested,
  Linux is home turf.

## Development

- `cargo run -- --smoke 5` — headless: acquire locks, print status, hold 5 s, exit. Useful for scripting/tests without a TTY.
- `cargo test` — run unit tests (power-source parsing, mode decision table, art layout invariants).
- `cargo clippy` — run linter checks.
- See `CLAUDE.md` for project rules (README updated every phase; clippy + tests must pass).

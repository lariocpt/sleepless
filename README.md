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

## Install

```sh
cargo install --path .
```

## Usage

```sh
nosleep                    # keep the machine awake while running
nosleep --plugged-only     # start in plugged-only mode
nosleep --inhibit-lid      # additionally block lid-close suspend
nosleep --no-tray          # no tray icon
nosleep --no-pulse         # no pulse animations
nosleep --why "compiling all of chromium"   # reason shown in systemd-inhibit --list
```

Keys: **m** toggle mode · **l** toggle lid-close block · **q** / Esc / Ctrl-C quit.

Check what it's holding from another terminal:

```sh
systemd-inhibit --list | grep nosleep
```

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

- `cargo run -- --smoke 5` — headless: acquire locks, print status, hold 5 s, exit.
  Useful for scripting/tests without a TTY.
- `cargo test` — power-source parsing (fake sysfs trees), mode decision table, art
  layout invariants.
- See `CLAUDE.md` for project rules (README updated every phase; clippy + tests
  must pass).

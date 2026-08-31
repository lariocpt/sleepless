//! The two ways to run: the full TUI, and the headless `--smoke` path that CI and
//! scripts use. Both share the same reconcile/revalidate cadence, so what CI
//! asserts is the same machinery the TUI runs.

use crate::app::App;
use crate::config::{self, State, StateWriter};
use crate::inhibit::{self, Buses};
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How often the power source is re-read and the held locks are re-validated.
const POLL_EVERY: Duration = Duration::from_secs(2);
/// One frame of the event loop.
const FRAME_MS: u64 = 250;
const FRAME: Duration = Duration::from_millis(FRAME_MS);

/// Pulse cadences, in frames and then in milliseconds. The website quotes these
/// numbers and its own animation runs at them, so they are named here rather than
/// left as bare `% 4` and `% 6` in two different files for the docs to guess at.
pub const SPLASH_PULSE_TICKS: u64 = 4;
pub const TRAY_PULSE_TICKS: u64 = 6;
pub const SPLASH_PULSE_MS: u64 = FRAME_MS * SPLASH_PULSE_TICKS / 2;
pub const TRAY_PULSE_MS: u64 = FRAME_MS * TRAY_PULSE_TICKS / 2;

/// What a key press means. Pulled out of the event loop so the keys the README
/// documents can be asserted against the keys the program actually handles --
/// inside a `match` in a draw loop, they were unreachable from any test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    ToggleMode,
    /// Only acted on where the platform can block the lid switch.
    ToggleLid,
    PrevSplash,
    NextSplash,
    Ignore,
}

pub fn action_for(code: KeyCode, mods: KeyModifiers) -> Action {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        // Raw mode means Ctrl-C arrives as a key, not a signal.
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => Action::Quit,
        KeyCode::Char('m') => Action::ToggleMode,
        KeyCode::Char('l') => Action::ToggleLid,
        KeyCode::Left => Action::PrevSplash,
        KeyCode::Right => Action::NextSplash,
        _ => Action::Ignore,
    }
}

pub fn tui(app: &mut App, buses: &Buses, tray_enabled: bool) -> Result<()> {
    // SIGTERM/SIGINT from outside: break the loop so the terminal is restored.
    // SIGHUP stays at its default (die immediately) -- the OS releases the locks.
    let term = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        let _ = signal_hook::flag::register(sig, Arc::clone(&term));
    }

    let mut tray = TrayCtl::start(app, tray_enabled);
    tray.push(app, false);

    let mut terminal = ratatui::init();
    let res = event_loop(&mut terminal, app, buses, &mut tray, &term);
    ratatui::restore();
    res
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    buses: &Buses,
    tray: &mut TrayCtl,
    term: &AtomicBool,
) -> Result<()> {
    let mut last_poll = Instant::now();
    let mut tray_frame = false;
    // Persist runtime toggles when they change, so the next launch restores them.
    // Seeded from the current state so nothing is written until the user actually
    // changes something.
    let mut writer = StateWriter::new(snapshot(app));
    loop {
        terminal.draw(|f| crate::ui::draw(f, app))?;

        let mut edge = false;
        if event::poll(FRAME)?
            && let Event::Key(k) = event::read()?
            && k.kind == KeyEventKind::Press
        {
            match action_for(k.code, k.modifiers) {
                Action::Quit => app.should_quit = true,
                Action::ToggleMode => {
                    app.toggle_mode();
                    app.reconcile(buses);
                    edge = true;
                }
                Action::ToggleLid if inhibit::CAPS.lid => {
                    app.toggle_lid();
                    app.reconcile(buses);
                    edge = true;
                }
                Action::PrevSplash => app.prev_splash(),
                Action::NextSplash => app.next_splash(),
                Action::ToggleLid | Action::Ignore => {}
            }
        }

        edge |= tray.pump(app, buses);

        app.tick = app.tick.wrapping_add(1);
        if last_poll.elapsed() >= POLL_EVERY {
            last_poll = Instant::now();
            edge |= poll_platform(app, buses);
        }

        // Tray pulse while active; only push on change or edge.
        let frame = app.pulse && app.active() && app.tick % TRAY_PULSE_TICKS < TRAY_PULSE_TICKS / 2;
        if edge || frame != tray_frame {
            tray_frame = frame;
            tray.push(app, frame);
        }

        if let Some(note) = writer.sync(Instant::now(), &snapshot(app), config::save_state) {
            app.config_notes.push(note);
        }

        if app.should_quit || term.load(Ordering::Relaxed) {
            return Ok(());
        }
    }
}

/// The once-every-two-seconds work both run modes share: re-read the power source,
/// notice a lock that has silently died, and retry an inhibit that failed.
/// Returns true if anything changed.
fn poll_platform(app: &mut App, buses: &Buses) -> bool {
    let mut changed = false;
    let p = crate::power::read();
    if p != app.power {
        app.power = p;
        app.reconcile(buses);
        changed = true;
    }
    changed |= app.revalidate(buses);
    app.retry_if_due(buses);
    changed
}

/// The persistable slice of runtime state.
fn snapshot(app: &App) -> State {
    State {
        mode: Some(app.mode.label().to_string()),
        inhibit_lid: Some(app.lid),
        splash: app.splash().map(|s| s.name.clone()),
    }
}

/// Owns the tray glue so the main loop reads the same with or without
/// the `tray` feature compiled in.
struct TrayCtl {
    #[cfg(all(target_os = "linux", feature = "tray"))]
    rx: std::sync::mpsc::Receiver<crate::tray::TrayMsg>,
    #[cfg(all(target_os = "linux", feature = "tray"))]
    handle: Option<crate::tray::Handle>,
}

impl TrayCtl {
    fn start(app: &mut App, enabled: bool) -> TrayCtl {
        #[cfg(all(target_os = "linux", feature = "tray"))]
        {
            use crate::tray;
            let (tx, rx) = std::sync::mpsc::channel();
            let handle = if enabled {
                match tray::spawn(tray::SleeplessTray::new(tx)) {
                    Ok(h) => Some(h),
                    Err(_) => {
                        app.tray_note = Some("tray: unavailable".into());
                        None
                    }
                }
            } else {
                None
            };
            TrayCtl { rx, handle }
        }
        #[cfg(not(all(target_os = "linux", feature = "tray")))]
        {
            let _ = (app, enabled);
            TrayCtl {}
        }
    }

    /// Drain tray commands; true if app state changed.
    fn pump(&mut self, app: &mut App, buses: &Buses) -> bool {
        #[cfg(all(target_os = "linux", feature = "tray"))]
        {
            use crate::tray::TrayMsg;
            let mut changed = false;
            for msg in self.rx.try_iter() {
                changed = true;
                match msg {
                    TrayMsg::SetMode(m) => app.set_mode(m),
                    TrayMsg::ToggleMode => app.toggle_mode(),
                    TrayMsg::SetLid(l) => app.set_lid(l),
                    TrayMsg::Quit => app.should_quit = true,
                }
            }
            if changed {
                app.reconcile(buses);
            }
            changed
        }
        #[cfg(not(all(target_os = "linux", feature = "tray")))]
        {
            let _ = (app, buses);
            false
        }
    }

    /// Push current state to the icon.
    fn push(&mut self, app: &mut App, frame: bool) {
        #[cfg(all(target_os = "linux", feature = "tray"))]
        if let Some(h) = &self.handle {
            let (mode, lid, active) = (app.mode, app.lid, app.active());
            let paused =
                !active && matches!(mode, crate::app::Mode::PluggedOnly) && !app.power.on_ac;
            let alive = h
                .update(move |t| {
                    t.mode = mode;
                    t.lid = lid;
                    t.active = active;
                    t.paused_on_battery = paused;
                    t.frame = frame;
                })
                .is_some();
            if !alive {
                self.handle = None;
                app.tray_note = Some("tray: exited".into());
            }
        }
        #[cfg(not(all(target_os = "linux", feature = "tray")))]
        {
            let _ = (app, frame);
        }
    }
}

/// Headless mode for scripts/tests: hold (and keep reconciling) for a while.
pub fn plain(app: &mut App, buses: &Buses, limit: Duration) -> Result<()> {
    let started = Instant::now();
    let mut last_active = app.active();
    let mut last_poll = Instant::now();
    while started.elapsed() < limit {
        std::thread::sleep(FRAME.min(limit));
        if last_poll.elapsed() >= POLL_EVERY {
            last_poll = Instant::now();
            poll_platform(app, buses);
        }
        if app.active() != last_active {
            last_active = app.active();
            print_state(app);
        }
    }
    Ok(())
}

/// The plain-ASCII status `--smoke` prints. Everything here has to survive being
/// piped through a `grep` in a CI script and a Windows console code page, so every
/// interpolated string goes through [`crate::ascii::squash`] first.
pub fn print_state(app: &App) {
    let state = if app.active() {
        format!("AWAKE ({})", app.mode.label())
    } else if app.inhibit_err.is_some() {
        "NOT INHIBITING (error)".into()
    } else {
        "PAUSED (on battery)".into()
    };
    println!(
        "sleepless - {state} | {}",
        crate::ascii::squash(&app.power.describe_ascii())
    );
    if let Some(g) = &app.guard {
        println!(
            "  holding: {}",
            crate::ascii::squash(&g.status().describe_ascii())
        );
        for note in &g.status().notes {
            println!("  note: {}", crate::ascii::squash(note));
        }
    }
    for note in &app.config_notes {
        println!("  {}", crate::ascii::squash(note));
    }
    if let Some(e) = &app.inhibit_err {
        println!("  error: {}", crate::ascii::squash(e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> Action {
        action_for(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn every_documented_key_does_something() {
        assert_eq!(key('q'), Action::Quit);
        assert_eq!(action_for(KeyCode::Esc, KeyModifiers::NONE), Action::Quit);
        assert_eq!(
            action_for(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Action::Quit,
            "raw mode means Ctrl-C is a key press, not a signal"
        );
        assert_eq!(key('m'), Action::ToggleMode);
        assert_eq!(key('l'), Action::ToggleLid);
        assert_eq!(
            action_for(KeyCode::Left, KeyModifiers::NONE),
            Action::PrevSplash
        );
        assert_eq!(
            action_for(KeyCode::Right, KeyModifiers::NONE),
            Action::NextSplash
        );
    }

    #[test]
    fn a_bare_c_is_not_quit() {
        assert_eq!(key('c'), Action::Ignore, "only Ctrl-C quits");
        assert_eq!(key('x'), Action::Ignore);
    }

    #[test]
    fn the_pulse_cadences_are_what_the_docs_quote() {
        assert_eq!(SPLASH_PULSE_MS, 500);
        assert_eq!(TRAY_PULSE_MS, 750);
    }
}

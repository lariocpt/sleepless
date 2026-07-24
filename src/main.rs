mod app;
mod art;
mod inhibit;
mod power;
#[cfg(all(target_os = "linux", feature = "tray"))]
mod tray;
mod ui;

use anyhow::Result;
use app::{App, Mode};
use clap::Parser;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Keep your computer awake while this runs. Close the terminal to stop.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Only keep awake while on AC power (toggle at runtime with 'm')
    #[arg(long)]
    plugged_only: bool,

    /// Also block lid-close suspend (logind handle-lid-switch)
    #[arg(long)]
    inhibit_lid: bool,

    /// Don't show a system tray icon
    #[arg(long)]
    no_tray: bool,

    /// Disable the pulse animation
    #[arg(long)]
    no_pulse: bool,

    /// Reason shown in `systemd-inhibit --list`
    #[arg(long, default_value = "I can't get no sleep")]
    why: String,

    /// Run headless for N seconds, print status, then exit
    #[arg(long, value_name = "SECS", hide = true)]
    smoke: Option<u64>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mode = if args.plugged_only {
        Mode::PluggedOnly
    } else {
        Mode::Always
    };
    let buses = inhibit::Buses::connect()?;
    let mut app = App::new(mode, args.inhibit_lid, !args.no_pulse, args.why.clone());
    app.power = power::read();
    app.reconcile(&buses);

    if let Some(secs) = args.smoke {
        print_state(&app);
        return run_plain(&mut app, &buses, Duration::from_secs(secs));
    }

    // SIGTERM/SIGINT from outside: break the loop so the terminal is restored.
    // SIGHUP stays at its default (die immediately) — the OS releases the locks.
    let term = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "linux")]
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        let _ = signal_hook::flag::register(sig, Arc::clone(&term));
    }

    let mut tray = TrayCtl::start(&mut app, !args.no_tray);
    tray.push(&mut app, false);

    let mut terminal = ratatui::init();
    let res = run_tui(&mut terminal, &mut app, &buses, &mut tray, &term);
    ratatui::restore();
    res
}

fn run_tui(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    buses: &inhibit::Buses,
    tray: &mut TrayCtl,
    term: &AtomicBool,
) -> Result<()> {
    let mut last_power = Instant::now();
    let mut tray_frame = false;
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        let mut edge = false;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(k) = event::read()?
            && k.kind == KeyEventKind::Press
        {
            match k.code {
                KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.should_quit = true;
                }
                KeyCode::Char('m') => {
                    app.toggle_mode();
                    app.reconcile(buses);
                    edge = true;
                }
                KeyCode::Char('l') => {
                    app.toggle_lid();
                    app.reconcile(buses);
                    edge = true;
                }
                _ => {}
            }
        }

        edge |= tray.pump(app, buses);

        app.tick = app.tick.wrapping_add(1);
        if last_power.elapsed() >= Duration::from_secs(2) {
            last_power = Instant::now();
            let p = power::read();
            if p != app.power {
                app.power = p;
                app.reconcile(buses);
                edge = true;
            }
            app.retry_if_due(buses);
        }

        // ~750 ms tray pulse while active; only push on change or edge.
        let frame = app.pulse && app.active() && app.tick % 6 < 3;
        if edge || frame != tray_frame {
            tray_frame = frame;
            tray.push(app, frame);
        }

        if app.should_quit || term.load(Ordering::Relaxed) {
            return Ok(());
        }
    }
}

/// Owns the tray glue so the main loop reads the same with or without
/// the `tray` feature compiled in.
struct TrayCtl {
    #[cfg(all(target_os = "linux", feature = "tray"))]
    rx: std::sync::mpsc::Receiver<tray::TrayMsg>,
    #[cfg(all(target_os = "linux", feature = "tray"))]
    handle: Option<tray::Handle>,
}

impl TrayCtl {
    fn start(app: &mut App, enabled: bool) -> TrayCtl {
        #[cfg(all(target_os = "linux", feature = "tray"))]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let handle = if enabled {
                match tray::spawn(tray::NosleepTray::new(tx)) {
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
    fn pump(&mut self, app: &mut App, buses: &inhibit::Buses) -> bool {
        #[cfg(all(target_os = "linux", feature = "tray"))]
        {
            let mut changed = false;
            for msg in self.rx.try_iter() {
                changed = true;
                match msg {
                    tray::TrayMsg::SetMode(m) => app.set_mode(m),
                    tray::TrayMsg::ToggleMode => app.toggle_mode(),
                    tray::TrayMsg::SetLid(l) => app.set_lid(l),
                    tray::TrayMsg::Quit => app.should_quit = true,
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
            let paused = !active && matches!(mode, Mode::PluggedOnly) && !app.power.on_ac;
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
fn run_plain(app: &mut App, buses: &inhibit::Buses, limit: Duration) -> Result<()> {
    let started = Instant::now();
    let mut last_active = app.active();
    while started.elapsed() < limit {
        std::thread::sleep(Duration::from_millis(500));
        let p = power::read();
        if p != app.power {
            app.power = p;
            app.reconcile(buses);
        }
        app.retry_if_due(buses);
        if app.active() != last_active {
            last_active = app.active();
            print_state(app);
        }
    }
    Ok(())
}

fn print_state(app: &App) {
    let state = if app.active() {
        format!("AWAKE ({})", app.mode.label())
    } else if app.inhibit_err.is_some() {
        "NOT INHIBITING (error)".into()
    } else {
        "PAUSED — on battery".into()
    };
    println!("nosleep — {state} · {}", app.power.describe());
    if let Some(g) = &app.guard {
        println!("  holding: {}", g.status().describe());
        for note in &g.status().notes {
            println!("  note: {note}");
        }
    }
    if let Some(e) = &app.inhibit_err {
        println!("  error: {e}");
    }
}

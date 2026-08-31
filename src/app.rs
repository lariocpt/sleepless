//! Application state machine, kept free of terminal/tray concerns.

use crate::art::Splash;
use crate::inhibit::{self, Buses, InhibitGuard};
use crate::power::PowerStatus;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Always,
    PluggedOnly,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Always => "always",
            Mode::PluggedOnly => "plugged-only",
        }
    }
}

/// The one rule: inhibit in Always mode, or in PluggedOnly mode while on AC.
pub fn desired_active(mode: Mode, power: &PowerStatus) -> bool {
    match mode {
        Mode::Always => true,
        Mode::PluggedOnly => power.on_ac,
    }
}

const RETRY_EVERY: Duration = Duration::from_secs(5);

pub struct App {
    pub mode: Mode,
    pub lid: bool,
    pub pulse: bool,
    pub why: String,
    pub power: PowerStatus,
    pub guard: Option<InhibitGuard>,
    pub inhibit_err: Option<String>,
    pub tray_note: Option<String>,
    pub splashes: Vec<Splash>,
    pub splash_idx: usize,
    pub config_notes: Vec<String>,
    pub tick: u64,
    pub should_quit: bool,
    last_attempt: Option<Instant>,
}

impl App {
    pub fn new(mode: Mode, lid: bool, pulse: bool, why: String) -> Self {
        Self {
            mode,
            lid,
            pulse,
            why,
            power: PowerStatus::default(),
            guard: None,
            inhibit_err: None,
            tray_note: None,
            splashes: Vec::new(),
            splash_idx: 0,
            config_notes: Vec::new(),
            tick: 0,
            should_quit: false,
            last_attempt: None,
        }
    }

    pub fn active(&self) -> bool {
        self.guard.is_some()
    }

    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Always => Mode::PluggedOnly,
            Mode::PluggedOnly => Mode::Always,
        };
    }

    /// Absolute setters, as opposed to the toggles: these exist for the tray
    /// menu's radio group and checkbox, which report a chosen state rather than
    /// a change. Nothing else calls them, so they are dead where no tray is built.
    #[cfg_attr(
        not(all(target_os = "linux", feature = "tray")),
        allow(dead_code, reason = "tray menu API; tray is Linux-only")
    )]
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub fn toggle_lid(&mut self) {
        self.lid = !self.lid;
    }

    #[cfg_attr(
        not(all(target_os = "linux", feature = "tray")),
        allow(dead_code, reason = "tray menu API; tray is Linux-only")
    )]
    pub fn set_lid(&mut self, lid: bool) {
        self.lid = lid;
    }

    pub fn splash(&self) -> Option<&Splash> {
        self.splashes.get(self.splash_idx)
    }

    pub fn next_splash(&mut self) {
        if !self.splashes.is_empty() {
            self.splash_idx = (self.splash_idx + 1) % self.splashes.len();
        }
    }

    pub fn prev_splash(&mut self) {
        if !self.splashes.is_empty() {
            self.splash_idx = (self.splash_idx + self.splashes.len() - 1) % self.splashes.len();
        }
    }

    /// Make reality match `desired_active`. Call on every edge:
    /// key press, tray command, power-source change.
    pub fn reconcile(&mut self, buses: &Buses) {
        let want = desired_active(self.mode, &self.power);
        let lid_mismatch = self
            .guard
            .as_ref()
            .is_some_and(|g| g.status().lid_requested != self.lid);
        if self.guard.is_some() && (!want || lid_mismatch) {
            self.guard = None; // Drop releases instantly
        }
        if !want {
            self.inhibit_err = None;
            self.last_attempt = None;
            return;
        }
        if self.guard.is_none() {
            match inhibit::acquire(buses, self.lid, &self.why) {
                Ok(g) => {
                    self.guard = Some(g);
                    self.inhibit_err = None;
                }
                Err(e) => self.inhibit_err = Some(e.to_string()),
            }
            self.last_attempt = Some(Instant::now());
        }
    }

    /// Notice a lock that has silently stopped being real -- the compositor or
    /// logind restarted -- and drop it so the normal acquire path takes over.
    /// Returns true if it did, so the caller can refresh the tray.
    ///
    /// Without this, `active()` is just `guard.is_some()`: the banner stayed green
    /// and [`retry_if_due`](Self::retry_if_due) never fired, because it only fires
    /// when there is no guard at all.
    pub fn revalidate(&mut self, buses: &Buses) -> bool {
        if self.guard.as_ref().is_some_and(|g| !g.is_live()) {
            self.guard = None;
            self.last_attempt = None; // reacquire now, not in five seconds
            self.reconcile(buses);
            return true;
        }
        false
    }

    /// Self-heal after transient D-Bus failures without hammering the bus.
    pub fn retry_if_due(&mut self, buses: &Buses) {
        if self.guard.is_none()
            && desired_active(self.mode, &self.power)
            && self.last_attempt.is_none_or(|t| t.elapsed() >= RETRY_EVERY)
        {
            self.reconcile(buses);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(on_ac: bool) -> PowerStatus {
        PowerStatus {
            on_ac,
            ..Default::default()
        }
    }

    #[test]
    fn decision_table() {
        assert!(desired_active(Mode::Always, &power(true)));
        assert!(desired_active(Mode::Always, &power(false)));
        assert!(desired_active(Mode::PluggedOnly, &power(true)));
        assert!(!desired_active(Mode::PluggedOnly, &power(false)));
    }

    #[test]
    fn mode_toggles_round_trip() {
        let mut app = App::new(Mode::Always, false, true, String::new());
        app.toggle_mode();
        assert_eq!(app.mode, Mode::PluggedOnly);
        app.toggle_mode();
        assert_eq!(app.mode, Mode::Always);
    }

    #[test]
    fn splash_cycling_wraps_both_ways() {
        let mut app = App::new(Mode::Always, false, true, String::new());
        app.splashes = crate::config::builtin_splashes();
        let n = app.splashes.len();
        assert!(n >= 3);
        for _ in 0..n {
            app.next_splash();
        }
        assert_eq!(app.splash_idx, 0, "forward cycle should wrap");
        app.prev_splash();
        assert_eq!(app.splash_idx, n - 1, "backward cycle should wrap");
        // Empty list must not panic or divide by zero.
        app.splashes.clear();
        app.next_splash();
        app.prev_splash();
        assert!(app.splash().is_none());
    }
}

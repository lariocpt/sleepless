//! Sleep/idle inhibition.
//!
//! Everything here is process-lifetime-bound, on every platform: logind hands us a
//! pipe fd that the kernel closes when we die, the ScreenSaver grant is tied to our
//! D-Bus connection (niri drops it on NameOwnerChanged), macOS powerd reaps
//! IOPMAssertions when their owning task dies, and Windows clears a thread's
//! execution state when the thread goes away. No cleanup is ever required — closing
//! the terminal, SIGKILL included, releases every lock.

/// What the platform backend is actually able to do, so the UI stops offering
/// controls that do nothing here.
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    /// Lid-close suspend can be blocked (logind `handle-lid-switch`).
    pub lid: bool,
    /// Name of the mechanism, for the footer and bug reports.
    pub mechanism: &'static str,
}

pub const CAPS: Caps = imp::CAPS;

/// Which locks are actually held right now, for the UI.
#[derive(Debug, Default, Clone)]
pub struct LockStatus {
    /// Idle/screensaver inhibit held (suppresses the compositor idle chain).
    pub idle: bool,
    /// System-sleep inhibit held.
    pub sleep: bool,
    /// `handle-lid-switch` included in the logind lock.
    pub lid: bool,
    /// What was asked for (may differ from `lid` if polkit refused it).
    pub lid_requested: bool,
    /// Human-readable partial-failure notes.
    pub notes: Vec<String>,
}

impl LockStatus {
    /// Plain-ASCII form for `--smoke` and anything a script will grep. The TUI
    /// uses [`describe`](Self::describe); this one has to survive being piped
    /// through a Windows console code page.
    pub fn describe_ascii(&self) -> String {
        let mark = |b: bool| if b { "yes" } else { "no" };
        let lid = if self.lid_requested && !self.lid {
            "refused"
        } else {
            mark(self.lid)
        };
        if CAPS.lid {
            format!(
                "idle={} sleep={} lid={}",
                mark(self.idle),
                mark(self.sleep),
                lid
            )
        } else {
            format!("idle={} sleep={}", mark(self.idle), mark(self.sleep))
        }
    }

    pub fn describe(&self) -> String {
        let mark = |b: bool| if b { "✓" } else { "✗" };
        let lid = if self.lid_requested && !self.lid {
            "✗ (refused)"
        } else if self.lid {
            "✓"
        } else {
            "✗"
        };
        // Only mention lid where blocking it is even possible.
        if CAPS.lid {
            format!(
                "idle {} · sleep {} · lid {}",
                mark(self.idle),
                mark(self.sleep),
                lid
            )
        } else {
            format!("idle {} · sleep {}", mark(self.idle), mark(self.sleep))
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Caps, LockStatus};
    use anyhow::{Result, bail};
    use std::os::fd::OwnedFd;
    use zbus::blocking::Connection;

    pub const CAPS: Caps = Caps {
        lid: true,
        mechanism: "D-Bus (ScreenSaver + logind)",
    };

    const SCREENSAVER: (&str, &str, &str) = (
        "org.freedesktop.ScreenSaver",
        "/org/freedesktop/ScreenSaver",
        "org.freedesktop.ScreenSaver",
    );

    /// Both bus connections, made once at startup. Either may be missing — and so
    /// may both: a bare TTY, a container or a non-systemd box still gets a running
    /// app that says `✗ NOT INHIBITING`, rather than a refusal to start.
    pub struct Buses {
        session: Option<Connection>,
        system: Option<Connection>,
        notes: Vec<String>,
    }

    impl Buses {
        pub fn connect() -> Buses {
            let mut notes = Vec::new();
            let session = match Connection::session() {
                Ok(c) => Some(c),
                Err(e) => {
                    notes.push(format!("session bus unavailable: {e}"));
                    None
                }
            };
            let system = match Connection::system() {
                Ok(c) => Some(c),
                Err(e) => {
                    notes.push(format!("system bus unavailable: {e}"));
                    None
                }
            };
            Buses {
                session,
                system,
                notes,
            }
        }
    }

    struct ScreenSaverLock {
        conn: Connection,
        cookie: u32,
    }

    /// RAII: dropping releases everything immediately; dying releases it anyway.
    pub struct InhibitGuard {
        screensaver: Option<ScreenSaverLock>,
        _logind: Option<OwnedFd>,
        status: LockStatus,
    }

    impl InhibitGuard {
        pub fn status(&self) -> &LockStatus {
            &self.status
        }
    }

    impl Drop for InhibitGuard {
        fn drop(&mut self) {
            // Best-effort immediate release; the fd closes on its own drop.
            if let Some(l) = self.screensaver.take() {
                let _ = l.conn.call_method(
                    Some(SCREENSAVER.0),
                    SCREENSAVER.1,
                    Some(SCREENSAVER.2),
                    "UnInhibit",
                    &(l.cookie,),
                );
            }
        }
    }

    fn screensaver_inhibit(conn: &Connection, why: &str) -> Result<u32, zbus::Error> {
        let reply = conn.call_method(
            Some(SCREENSAVER.0),
            SCREENSAVER.1,
            Some(SCREENSAVER.2),
            "Inhibit",
            &("nosleep", why),
        )?;
        reply.body().deserialize::<u32>()
    }

    fn logind_inhibit(conn: &Connection, what: &str, why: &str) -> Result<OwnedFd, zbus::Error> {
        let reply = conn.call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "Inhibit",
            &(what, "nosleep", why, "block"),
        )?;
        let fd: zbus::zvariant::OwnedFd = reply.body().deserialize()?;
        Ok(fd.into())
    }

    /// Acquire as much as possible; Ok if at least one lock is held.
    pub fn acquire(buses: &Buses, lid: bool, why: &str) -> Result<InhibitGuard> {
        let mut status = LockStatus {
            lid_requested: lid,
            ..Default::default()
        };
        status.notes.extend(buses.notes.iter().cloned());

        let screensaver =
            buses
                .session
                .as_ref()
                .and_then(|conn| match screensaver_inhibit(conn, why) {
                    Ok(cookie) => {
                        status.idle = true;
                        Some(ScreenSaverLock {
                            conn: conn.clone(),
                            cookie,
                        })
                    }
                    Err(e) => {
                        status
                            .notes
                            .push(format!("ScreenSaver inhibit failed: {e}"));
                        None
                    }
                });

        let logind = buses.system.as_ref().and_then(|conn| {
            let what = if lid {
                "sleep:idle:handle-lid-switch"
            } else {
                "sleep:idle"
            };
            match logind_inhibit(conn, what, why) {
                Ok(fd) => {
                    status.sleep = true;
                    status.lid = lid;
                    Some(fd)
                }
                // Polkit may allow sleep but refuse handle-lid-switch: degrade.
                Err(e) if lid => {
                    status.notes.push(format!(
                        "lid-switch inhibit refused ({e}); holding sleep:idle only"
                    ));
                    match logind_inhibit(conn, "sleep:idle", why) {
                        Ok(fd) => {
                            status.sleep = true;
                            Some(fd)
                        }
                        Err(e2) => {
                            status.notes.push(format!("logind inhibit failed: {e2}"));
                            None
                        }
                    }
                }
                Err(e) => {
                    status.notes.push(format!("logind inhibit failed: {e}"));
                    None
                }
            }
        });

        if screensaver.is_none() && logind.is_none() {
            bail!(
                "could not acquire any inhibitor: {}",
                status.notes.join("; ")
            );
        }
        Ok(InhibitGuard {
            screensaver,
            _logind: logind,
            status,
        })
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod imp {
    //! keepawake wraps `IOPMAssertionCreateWithName` on macOS and
    //! `SetThreadExecutionState` on Windows. Both are process-scoped, so the
    //! core guarantee holds; neither spawns a subprocess.
    //!
    //! Option mapping matters here and is easy to get wrong:
    //!   * `display` -> `PreventUserIdleDisplaySleep` / `ES_DISPLAY_REQUIRED`.
    //!     We want this: on Linux `org.freedesktop.ScreenSaver.Inhibit` also
    //!     suppresses blanking, so leaving it off made the same banner mean
    //!     two different things depending on the OS.
    //!   * `idle` -> `PreventUserIdleSystemSleep` / `ES_SYSTEM_REQUIRED`. This is
    //!     the load-bearing one on both platforms.
    //!   * `sleep` -> `PreventSystemSleep` on macOS (valid on AC only), but
    //!     `ES_AWAYMODE_REQUIRED` on Windows, which Microsoft documents as
    //!     Traditional-Sleep-only and explicitly not for portable machines.
    //!     So we ask for it on macOS and never on Windows.
    use super::{Caps, LockStatus};
    use anyhow::Result;

    pub const CAPS: Caps = Caps {
        lid: false,
        #[cfg(target_os = "macos")]
        mechanism: "IOPMAssertion",
        #[cfg(target_os = "windows")]
        mechanism: "SetThreadExecutionState",
    };

    const WANT_SYSTEM_SLEEP_ASSERTION: bool = cfg!(target_os = "macos");

    pub struct Buses;

    impl Buses {
        pub fn connect() -> Buses {
            Buses
        }
    }

    pub struct InhibitGuard {
        _awake: keepawake::KeepAwake,
        status: LockStatus,
    }

    impl InhibitGuard {
        pub fn status(&self) -> &LockStatus {
            &self.status
        }
    }

    pub fn acquire(_buses: &Buses, lid: bool, why: &str) -> Result<InhibitGuard> {
        let awake = keepawake::Builder::default()
            .display(true)
            .idle(true)
            .sleep(WANT_SYSTEM_SLEEP_ASSERTION)
            .reason(why)
            .app_name("nosleep")
            .app_reverse_domain("dev.lario.nosleep")
            .create()?;

        // Only claim what we actually asked for and got. `create()` fails as a
        // whole if any requested assertion fails, so reaching here means every
        // requested lock is held — but `sleep` is not requested on Windows.
        let mut status = LockStatus {
            idle: true,
            sleep: WANT_SYSTEM_SLEEP_ASSERTION,
            lid: false,
            lid_requested: lid,
            notes: Vec::new(),
        };
        if lid {
            status
                .notes
                .push("lid-close blocking is not available on this platform".into());
        }
        if cfg!(target_os = "macos") {
            status
                .notes
                .push("PreventSystemSleep applies on AC power only".into());
        }
        if cfg!(target_os = "windows") {
            status.notes.push(
                "on Modern Standby laptops Windows may still sleep on battery; \
                 --plugged-only is the dependable mode"
                    .into(),
            );
        }
        Ok(InhibitGuard {
            _awake: awake,
            status,
        })
    }
}

// FreeBSD, illumos and friends: keepawake has no backend, so say so plainly
// instead of failing at runtime with something cryptic.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod imp {
    use super::{Caps, LockStatus};
    use anyhow::{Result, bail};

    pub const CAPS: Caps = Caps {
        lid: false,
        mechanism: "unsupported",
    };

    pub struct Buses;

    impl Buses {
        pub fn connect() -> Buses {
            Buses
        }
    }

    pub struct InhibitGuard {
        status: LockStatus,
    }

    impl InhibitGuard {
        pub fn status(&self) -> &LockStatus {
            &self.status
        }
    }

    pub fn acquire(_buses: &Buses, _lid: bool, _why: &str) -> Result<InhibitGuard> {
        bail!("no sleep-inhibition backend for this platform")
    }
}

pub use imp::{Buses, InhibitGuard, acquire};

// Catch signature drift between the backends at compile time on every target.
// Cheaper and clearer than a trait, given exactly one arm is ever compiled.
const _: fn() -> Buses = Buses::connect;
const _: fn(&Buses, bool, &str) -> anyhow::Result<InhibitGuard> = acquire;
const _: fn(&InhibitGuard) -> &LockStatus = InhibitGuard::status;

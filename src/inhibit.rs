//! Sleep/idle inhibition.
//!
//! Everything here is process-lifetime-bound: logind hands us a pipe fd that the
//! kernel closes when we die, and the ScreenSaver grant is tied to our D-Bus
//! connection (niri drops it on NameOwnerChanged). No cleanup is ever required —
//! closing the terminal, SIGKILL included, releases every lock.

/// Which locks are actually held right now, for the UI.
#[derive(Debug, Default, Clone)]
pub struct LockStatus {
    /// fd.o ScreenSaver inhibit held (suppresses the compositor idle chain).
    pub idle: bool,
    /// logind `sleep:idle` block held.
    pub sleep: bool,
    /// `handle-lid-switch` included in the logind lock.
    pub lid: bool,
    /// What was asked for (may differ from `lid` if polkit refused it).
    pub lid_requested: bool,
    /// Human-readable partial-failure notes.
    pub notes: Vec<String>,
}

impl LockStatus {
    pub fn describe(&self) -> String {
        let mark = |b: bool| if b { "✓" } else { "✗" };
        let lid = if self.lid_requested && !self.lid {
            "✗ (refused)"
        } else if self.lid {
            "✓"
        } else {
            "✗"
        };
        format!(
            "idle {} · sleep {} · lid {}",
            mark(self.idle),
            mark(self.sleep),
            lid
        )
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::LockStatus;
    use anyhow::{Result, bail};
    use std::os::fd::OwnedFd;
    use zbus::blocking::Connection;

    const SCREENSAVER: (&str, &str, &str) = (
        "org.freedesktop.ScreenSaver",
        "/org/freedesktop/ScreenSaver",
        "org.freedesktop.ScreenSaver",
    );

    /// Both bus connections, made once at startup. Either may be missing;
    /// `acquire` works with whatever is reachable.
    pub struct Buses {
        session: Option<Connection>,
        system: Option<Connection>,
        notes: Vec<String>,
    }

    impl Buses {
        pub fn connect() -> Result<Buses> {
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
            if session.is_none() && system.is_none() {
                bail!("no D-Bus connection possible: {}", notes.join("; "));
            }
            Ok(Buses {
                session,
                system,
                notes,
            })
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

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::LockStatus;
    use anyhow::Result;

    pub struct Buses;

    impl Buses {
        pub fn connect() -> Result<Buses> {
            Ok(Buses)
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
            .display(false)
            .idle(true)
            .sleep(true)
            .reason(why)
            .app_name("nosleep")
            .app_reverse_domain("dev.lario.nosleep")
            .create()?;
        let mut status = LockStatus {
            idle: true,
            sleep: true,
            lid_requested: lid,
            ..Default::default()
        };
        if lid {
            status.notes.push("lid-close control is Linux-only".into());
        }
        Ok(InhibitGuard {
            _awake: awake,
            status,
        })
    }
}

pub use imp::{Buses, InhibitGuard, acquire};

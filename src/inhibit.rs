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
        mechanism: "D-Bus",
    };

    const SCREENSAVER: (&str, &str, &str) = (
        "org.freedesktop.ScreenSaver",
        "/org/freedesktop/ScreenSaver",
        "org.freedesktop.ScreenSaver",
    );

    const LOGIND: (&str, &str, &str) = (
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    );

    /// Which unique bus name owned a well-known name when we took a lock from it.
    ///
    /// This is the whole of the liveness check, and it exists because losing a lock
    /// is silent. If the compositor or logind restarts, the cookie names a session
    /// that no longer exists and the pipe fd has nobody on the other end -- but our
    /// connection stays up and the fd stays valid, so `guard.is_some()` kept the
    /// banner green and the retry path never ran, because it only fires when there
    /// is no guard at all. A restarted service takes a new unique name, so
    /// comparing owners catches exactly that.
    fn name_owner(conn: &Connection, name: &str) -> Option<String> {
        conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(name,),
        )
        .ok()?
        .body()
        .deserialize::<String>()
        .ok()
    }

    /// Is a lock taken from `recorded` still held, now that the name is owned by
    /// `current`? An owner that could not be read at acquire time is never treated
    /// as lost: dropping a working lock because the bookkeeping is missing would be
    /// worse than the bug this catches.
    fn still_owned(recorded: Option<&str>, current: Option<&str>) -> bool {
        match recorded {
            None => true,
            Some(r) => current == Some(r),
        }
    }

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
        owner: Option<String>,
    }

    struct LogindLock {
        conn: Connection,
        /// The lock lasts exactly as long as this fd is open. Never touched again;
        /// dropping it, or dying, is what releases it.
        _fd: OwnedFd,
        owner: Option<String>,
    }

    /// RAII: dropping releases everything immediately; dying releases it anyway.
    pub struct InhibitGuard {
        screensaver: Option<ScreenSaverLock>,
        logind: Option<LogindLock>,
        status: LockStatus,
    }

    impl InhibitGuard {
        pub fn status(&self) -> &LockStatus {
            &self.status
        }

        /// Is every lock this guard claims to hold still real?
        ///
        /// False means the owning service restarted, so the caller drops the guard
        /// and re-acquires. Re-acquiring records the new owners, and only claims
        /// what it actually got -- so a provider that is permanently gone settles
        /// into a guard that no longer mentions it, rather than churning.
        pub fn is_live(&self) -> bool {
            let ok = |conn: &Connection, name: &str, owner: &Option<String>| {
                still_owned(owner.as_deref(), name_owner(conn, name).as_deref())
            };
            self.screensaver
                .as_ref()
                .is_none_or(|l| ok(&l.conn, SCREENSAVER.0, &l.owner))
                && self
                    .logind
                    .as_ref()
                    .is_none_or(|l| ok(&l.conn, LOGIND.0, &l.owner))
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
            &("sleepless", why),
        )?;
        reply.body().deserialize::<u32>()
    }

    fn logind_inhibit(conn: &Connection, what: &str, why: &str) -> Result<OwnedFd, zbus::Error> {
        let reply = conn.call_method(
            Some(LOGIND.0),
            LOGIND.1,
            Some(LOGIND.2),
            "Inhibit",
            &(what, "sleepless", why, "block"),
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
                            owner: name_owner(conn, SCREENSAVER.0),
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
            let fd = match logind_inhibit(conn, what, why) {
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
            };
            fd.map(|fd| LogindLock {
                owner: name_owner(conn, LOGIND.0),
                conn: conn.clone(),
                _fd: fd,
            })
        });

        if screensaver.is_none() && logind.is_none() {
            bail!(
                "could not acquire any inhibitor: {}",
                status.notes.join("; ")
            );
        }
        Ok(InhibitGuard {
            screensaver,
            logind,
            status,
        })
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::{BufRead, BufReader};
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        #[test]
        fn still_owned_rules() {
            assert!(
                still_owned(Some(":1.7"), Some(":1.7")),
                "same owner: still ours"
            );
            assert!(
                !still_owned(Some(":1.7"), Some(":1.9")),
                "restarted service: a new unique name, so the lock is gone"
            );
            assert!(
                !still_owned(Some(":1.7"), None),
                "nobody owns it: the lock is gone"
            );
            // Never churn a working lock over bookkeeping we failed to record.
            assert!(still_owned(None, None));
            assert!(still_owned(None, Some(":1.9")));
        }

        struct Daemon(Child);
        impl Drop for Daemon {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        /// A private session bus, or None when dbus-daemon is not installed. A build
        /// host without D-Bus announces a skip rather than failing: this layer is
        /// about the bus, and "no bus here" is not a broken lock.
        fn private_bus() -> Option<(Daemon, String)> {
            let mut child = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let mut line = String::new();
            BufReader::new(child.stdout.take()?)
                .read_line(&mut line)
                .ok()?;
            let addr = line.trim().to_string();
            if addr.is_empty() {
                return None;
            }
            Some((Daemon(child), addr))
        }

        struct StubScreenSaver;

        #[zbus::interface(name = "org.freedesktop.ScreenSaver")]
        impl StubScreenSaver {
            fn inhibit(&self, _app: &str, _why: &str) -> u32 {
                42
            }
            fn un_inhibit(&self, _cookie: u32) {}
        }

        #[test]
        fn a_lock_stops_being_live_when_its_owner_goes_away() {
            let Some((_daemon, addr)) = private_bus() else {
                eprintln!("SKIP: dbus-daemon is not installed");
                return;
            };
            let connect = |b: zbus::blocking::connection::Builder<'_>| b.build().unwrap();

            let provider = connect(
                zbus::blocking::connection::Builder::address(addr.as_str())
                    .unwrap()
                    .name(SCREENSAVER.0)
                    .unwrap()
                    .serve_at(SCREENSAVER.1, StubScreenSaver)
                    .unwrap(),
            );
            let client =
                connect(zbus::blocking::connection::Builder::address(addr.as_str()).unwrap());

            // A real Inhibit call against a real bus, exactly as acquire() makes it.
            let cookie = screensaver_inhibit(&client, "testing").expect("stub should answer");
            let owner = name_owner(&client, SCREENSAVER.0);
            assert!(owner.is_some(), "the provider owns the name");

            let guard = InhibitGuard {
                screensaver: Some(ScreenSaverLock {
                    conn: client.clone(),
                    cookie,
                    owner,
                }),
                logind: None,
                status: LockStatus::default(),
            };
            assert!(guard.is_live(), "the provider is still there");

            // The provider goes away -- a compositor restart, from our side. Nothing
            // tells us: our connection is fine and the cookie is still a number.
            drop(provider);
            let deadline = Instant::now() + Duration::from_secs(5);
            while guard.is_live() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(25));
            }
            assert!(
                !guard.is_live(),
                "a lock whose owner has gone must not keep reporting itself held"
            );
        }
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
        mechanism: "ExecutionState",
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

        /// Always true here: an IOPMAssertion and a thread execution state belong
        /// to this process and cannot be revoked out from under it the way a D-Bus
        /// name can. They end when the process does, which is the point.
        pub fn is_live(&self) -> bool {
            true
        }
    }

    pub fn acquire(_buses: &Buses, lid: bool, why: &str) -> Result<InhibitGuard> {
        let awake = keepawake::Builder::default()
            .display(true)
            .idle(true)
            .sleep(WANT_SYSTEM_SLEEP_ASSERTION)
            .reason(why)
            .app_name("sleepless")
            .app_reverse_domain("dev.lario.sleepless")
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

        /// Unreachable: `acquire` never returns a guard on this platform.
        pub fn is_live(&self) -> bool {
            true
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
const _: fn(&InhibitGuard) -> bool = InhibitGuard::is_live;

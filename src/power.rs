//! Power-source detection.
//!
//! No emoji in the rendered strings, deliberately. `🔋` (U+1F50B) needs a colour
//! emoji font that minimal systems and bare TTYs do not have, and both it and `⚡`
//! carry emoji presentation, so terminals draw them two cells wide while the
//! layout math in `art.rs` counts them as one and the centring drifts. The words
//! already say what the icons said.
//!
//! Linux reads sysfs directly: any `type == Mains` supply that is `online`
//! means we're on AC. Battery percentage/status are display-only — charge-limit
//! firmware reports states like "Not charging" that must never be interpreted
//! as an AC signal. Peripheral batteries (wireless mice etc., `scope == Device`)
//! are ignored.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerStatus {
    pub on_ac: bool,
    pub battery_percent: Option<u8>,
    /// Raw kernel status string ("Charging", "Discharging", "Not charging", …).
    pub battery_state: Option<String>,
}

impl Default for PowerStatus {
    fn default() -> Self {
        Self {
            on_ac: true,
            battery_percent: None,
            battery_state: None,
        }
    }
}

impl PowerStatus {
    /// Plain-ASCII form for `--smoke` and anything a script will grep.
    pub fn describe_ascii(&self) -> String {
        let batt = self.battery_percent.map(|p| match &self.battery_state {
            Some(st) => format!("battery {p}% ({st})"),
            None => format!("battery {p}%"),
        });
        match (self.on_ac, batt) {
            (true, Some(b)) => format!("AC - {b}"),
            (true, None) => "AC".into(),
            (false, Some(b)) => format!("BAT - {b}"),
            (false, None) => "on battery".into(),
        }
    }

    pub fn describe(&self) -> String {
        let batt = self.battery_percent.map(|p| match &self.battery_state {
            Some(st) => format!("battery {p}% ({st})"),
            None => format!("battery {p}%"),
        });
        match (self.on_ac, batt) {
            (true, Some(b)) => format!("AC — {b}"),
            (true, None) => "AC".into(),
            (false, Some(b)) => b,
            (false, None) => "on battery".into(),
        }
    }
}

#[cfg(target_os = "linux")]
pub fn read() -> PowerStatus {
    read_from(std::path::Path::new("/sys/class/power_supply"))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn read_from(root: &std::path::Path) -> PowerStatus {
    let mut mains_seen = false;
    let mut mains_online = false;
    let mut batt_pct: Option<u8> = None;
    let mut batt_state: Option<String> = None;

    if let Ok(entries) = std::fs::read_dir(root) {
        let mut supplies: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        supplies.sort(); // deterministic: BAT0 wins over BAT1
        for p in supplies {
            let read = |f: &str| {
                std::fs::read_to_string(p.join(f))
                    .ok()
                    .map(|s| s.trim().to_string())
            };
            if read("scope").as_deref() == Some("Device") {
                continue;
            }
            match read("type").as_deref() {
                Some("Mains") => {
                    mains_seen = true;
                    if read("online").as_deref() == Some("1") {
                        mains_online = true;
                    }
                }
                Some("Battery") => {
                    if batt_pct.is_none() {
                        batt_pct = read("capacity").and_then(|s| s.parse().ok());
                    }
                    if batt_state.is_none() {
                        batt_state = read("status").filter(|s| !s.is_empty());
                    }
                }
                _ => {}
            }
        }
    }

    let on_ac = if mains_seen {
        mains_online
    } else if let Some(st) = &batt_state {
        // Rare ACPI quirk: battery driver but no Mains device.
        st != "Discharging"
    } else {
        // No system power supplies at all: a desktop. Always "plugged in".
        true
    };

    PowerStatus {
        on_ac,
        battery_percent: batt_pct,
        battery_state: batt_state,
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn read() -> PowerStatus {
    use starship_battery::{Manager, State};
    let mut out = PowerStatus::default();
    let Ok(manager) = Manager::new() else {
        return out;
    };
    let Ok(batteries) = manager.batteries() else {
        return out;
    };
    if let Some(b) = batteries.flatten().next() {
        let pct = (b.state_of_charge().value * 100.0)
            .round()
            .clamp(0.0, 100.0);
        out.battery_percent = Some(pct as u8);
        out.battery_state = Some(format!("{:?}", b.state()));
        // Best-effort heuristic; there is no AC concept in the crate.
        out.on_ac = !matches!(b.state(), State::Discharging | State::Empty);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn fake(root: &Path, name: &str, files: &[(&str, &str)]) {
        let d = root.join(name);
        fs::create_dir_all(&d).unwrap();
        for (f, v) in files {
            fs::write(d.join(f), v).unwrap();
        }
    }

    #[test]
    fn laptop_on_ac() {
        let t = tempfile::tempdir().unwrap();
        fake(t.path(), "ADP1", &[("type", "Mains\n"), ("online", "1\n")]);
        fake(
            t.path(),
            "BAT0",
            &[
                ("type", "Battery\n"),
                ("capacity", "99\n"),
                ("status", "Charging\n"),
            ],
        );
        let p = read_from(t.path());
        assert!(p.on_ac);
        assert_eq!(p.battery_percent, Some(99));
        assert_eq!(p.battery_state.as_deref(), Some("Charging"));
    }

    #[test]
    fn laptop_on_battery() {
        let t = tempfile::tempdir().unwrap();
        fake(t.path(), "ADP1", &[("type", "Mains\n"), ("online", "0\n")]);
        fake(
            t.path(),
            "BAT0",
            &[
                ("type", "Battery\n"),
                ("capacity", "84\n"),
                ("status", "Discharging\n"),
            ],
        );
        let p = read_from(t.path());
        assert!(!p.on_ac);
        assert_eq!(p.battery_percent, Some(84));
    }

    #[test]
    fn charge_limited_battery_is_not_an_ac_signal() {
        // "Not charging" at a charge cap, but the Mains supply decides.
        let t = tempfile::tempdir().unwrap();
        fake(t.path(), "ADP1", &[("type", "Mains\n"), ("online", "0\n")]);
        fake(
            t.path(),
            "BAT0",
            &[
                ("type", "Battery\n"),
                ("capacity", "80\n"),
                ("status", "Not charging\n"),
            ],
        );
        assert!(!read_from(t.path()).on_ac);
    }

    #[test]
    fn desktop_without_supplies_counts_as_ac() {
        let t = tempfile::tempdir().unwrap();
        assert!(read_from(t.path()).on_ac);
    }

    #[test]
    fn peripheral_battery_is_ignored() {
        let t = tempfile::tempdir().unwrap();
        fake(
            t.path(),
            "hidpp_battery_0",
            &[
                ("type", "Battery\n"),
                ("capacity", "15\n"),
                ("status", "Discharging\n"),
                ("scope", "Device\n"),
            ],
        );
        let p = read_from(t.path());
        assert!(p.on_ac);
        assert_eq!(p.battery_percent, None);
    }

    #[test]
    fn no_mains_driver_falls_back_to_battery_status() {
        let t = tempfile::tempdir().unwrap();
        fake(
            t.path(),
            "BAT0",
            &[
                ("type", "Battery\n"),
                ("capacity", "50\n"),
                ("status", "Discharging\n"),
            ],
        );
        assert!(!read_from(t.path()).on_ac);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn read() -> PowerStatus {
    PowerStatus::default()
}

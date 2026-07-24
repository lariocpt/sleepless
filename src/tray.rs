//! System tray icon: a StatusNotifierItem served by ksni on its own thread.
//!
//! ksni callbacks run inside ksni's async service thread, where our blocking
//! zbus connections must never be used — so callbacks only send `TrayMsg`
//! over the channel. Authoritative state flows back in via `Handle::update`.

use crate::app::Mode;
use std::sync::mpsc::Sender;

pub enum TrayMsg {
    SetMode(Mode),
    ToggleMode,
    SetLid(bool),
    Quit,
}

pub struct NosleepTray {
    pub mode: Mode,
    pub lid: bool,
    pub active: bool,
    pub paused_on_battery: bool,
    /// Pulse frame: alternates while active for extra visibility.
    pub frame: bool,
    tx: Sender<TrayMsg>,
}

impl NosleepTray {
    pub fn new(tx: Sender<TrayMsg>) -> Self {
        Self {
            mode: Mode::Always,
            lid: false,
            active: false,
            paused_on_battery: false,
            frame: false,
            tx,
        }
    }
}

impl ksni::Tray for NosleepTray {
    fn id(&self) -> String {
        "nosleep".into()
    }

    fn title(&self) -> String {
        if self.active {
            format!("nosleep — AWAKE ({})", self.mode.label())
        } else if self.paused_on_battery {
            "nosleep — PAUSED (on battery)".into()
        } else {
            "nosleep — not inhibiting".into()
        }
    }

    fn status(&self) -> ksni::Status {
        // Hosts that support it highlight NeedsAttention items.
        if self.paused_on_battery {
            ksni::Status::NeedsAttention
        } else {
            ksni::Status::Active
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [22, 32, 48]
            .into_iter()
            .map(|s| make_icon(s, self.active, self.frame))
            .collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.title(),
            description:
                "Left-click: toggle mode. Closing the nosleep terminal restores normal sleep."
                    .into(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            RadioGroup {
                selected: match self.mode {
                    Mode::Always => 0,
                    Mode::PluggedOnly => 1,
                },
                select: Box::new(|t: &mut Self, i| {
                    let mode = if i == 1 {
                        Mode::PluggedOnly
                    } else {
                        Mode::Always
                    };
                    let _ = t.tx.send(TrayMsg::SetMode(mode));
                }),
                options: vec![
                    RadioItem {
                        label: "Keep awake: always".into(),
                        ..Default::default()
                    },
                    RadioItem {
                        label: "Keep awake: only while plugged in".into(),
                        ..Default::default()
                    },
                ],
            }
            .into(),
            CheckmarkItem {
                label: "Block lid-close suspend".into(),
                checked: self.lid,
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(TrayMsg::SetLid(!t.lid));
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit nosleep".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(TrayMsg::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayMsg::ToggleMode);
    }

    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        // Keep serving so the icon reappears when the bar (Quickshell) restarts.
        true
    }
}

pub type Handle = ksni::blocking::Handle<NosleepTray>;

pub fn spawn(tray: NosleepTray) -> Result<Handle, ksni::Error> {
    use ksni::blocking::TrayMethods;
    tray.spawn()
}

/// High-contrast eye icon, drawn per-pixel (ARGB32, network byte order).
/// Active: bright green + open eye. Inactive/paused: amber + closed eye.
fn make_icon(size: i32, active: bool, alt_frame: bool) -> ksni::Icon {
    let s = size as usize;
    let mut data = vec![0u8; s * s * 4];
    let put = |d: &mut Vec<u8>, x: usize, y: usize, rgb: (u8, u8, u8)| {
        let i = (y * s + x) * 4;
        d[i] = 0xff;
        d[i + 1] = rgb.0;
        d[i + 2] = rgb.1;
        d[i + 3] = rgb.2;
    };

    let bg = if active {
        if alt_frame {
            (0x4a, 0xde, 0x80) // lighter green pulse frame
        } else {
            (0x16, 0xa3, 0x4a) // saturated green
        }
    } else {
        (0xf5, 0x9e, 0x0b) // amber
    };

    let c = (s as f64 - 1.0) / 2.0;
    let half = s as f64 / 2.0 - 0.5;
    let corner = s as f64 * 0.24;

    // Rounded-square background.
    for y in 0..s {
        for x in 0..s {
            let dx = (x as f64 - c).abs();
            let dy = (y as f64 - c).abs();
            let qx = (dx - (half - corner)).max(0.0);
            let qy = (dy - (half - corner)).max(0.0);
            let inside = dx <= half && dy <= half && qx.hypot(qy) <= corner;
            if inside {
                put(&mut data, x, y, bg);
            }
        }
    }

    let ex = s as f64 * 0.36; // eye x radius
    let ey = s as f64 * 0.22; // eye y radius
    if active {
        // Open eye: white sclera, near-black pupil.
        for y in 0..s {
            for x in 0..s {
                let nx = (x as f64 - c) / ex;
                let ny = (y as f64 - c) / ey;
                if nx * nx + ny * ny <= 1.0 {
                    put(&mut data, x, y, (0xff, 0xff, 0xff));
                }
            }
        }
        let pr = s as f64 * 0.12;
        for y in 0..s {
            for x in 0..s {
                if (x as f64 - c).hypot(y as f64 - c) <= pr {
                    put(&mut data, x, y, (0x08, 0x08, 0x08));
                }
            }
        }
    } else {
        // Closed eye: thick dark lid with three lashes.
        let dark = (0x33, 0x1c, 0x00);
        let lw = (s as f64 * 0.07).max(1.0);
        for y in 0..s {
            for x in 0..s {
                let nx = (x as f64 - c) / ex;
                let dy = y as f64 - c;
                if nx.abs() <= 1.0 && dy.abs() <= lw {
                    put(&mut data, x, y, dark);
                }
            }
        }
        let lash_len = (s as f64 * 0.14).max(2.0);
        for nx in [-0.62_f64, 0.0, 0.62] {
            let lx = (c + nx * ex).round() as isize;
            for dy in 0..(lash_len as isize) {
                let y = (c + lw).round() as isize + dy;
                for dx in -(lw as isize).max(1) / 2..=(lw as isize).max(1) / 2 {
                    let x = lx + dx;
                    if x >= 0 && (x as usize) < s && y >= 0 && (y as usize) < s {
                        put(&mut data, x as usize, y as usize, dark);
                    }
                }
            }
        }
    }

    ksni::Icon {
        width: size,
        height: size,
        data,
    }
}

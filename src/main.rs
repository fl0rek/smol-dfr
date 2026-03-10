use chrono::{Local, Locale, Timelike, format::{StrftimeItems, Item as ChronoItem}};
use drm::control::ClipRect;
use input::{
    event::{
        device::DeviceEvent,
        keyboard::{KeyState, KeyboardEvent, KeyboardEventTrait},
        touch::{TouchEvent, TouchEventPosition, TouchEventSlot},
        Event, EventTrait,
    },
    Device as InputDevice, Libinput, LibinputInterface,
};
use input_linux::{uinput::UInputHandle, EventKind, Key, SynchronizeKind};
use input_linux_sys::{input_event, input_id, timeval, uinput_setup};
use libc::{c_char, O_ACCMODE, O_RDONLY, O_RDWR, O_WRONLY};
use nix::{
    errno::Errno,
    sys::{
        epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags},
        signal::{SigSet, Signal},
    },
};
use privdrop::PrivDrop;
use std::{
    cmp::min,
    collections::HashMap,
    fs::{self, File, OpenOptions},
    os::{
        fd::{AsFd, AsRawFd},
        unix::{fs::OpenOptionsExt, io::OwnedFd},
    },
    panic::{self, AssertUnwindSafe},
    path::Path,
    time::Instant,
};
use udev::MonitorBuilder;

mod backlight;
mod battery_icon_widget;
mod config;
mod display;
mod iced_renderer;
mod memory_graph;
mod memory_graph_widget;
mod pixel_shift;
mod reconnect;
mod session_detect;
mod volume;
mod workspace;

use crate::config::ConfigManager;
use backlight::BacklightManager;
use config::{ButtonConfig, WorkspacesConfig};
use display::DrmBackend;
use iced_renderer::{BatteryInfo, ButtonAction, ButtonDef, Message as IcedMessage, TouchbarRenderer};
use memory_graph::MemoryHistory;
use pixel_shift::PixelShiftManager;
use reconnect::ReconnectWatcher;
use volume::VolumeManager;
use workspace::WorkspaceManager;

const TIMEOUT_MS: i32 = 10 * 1000;
const SYSFS_RETRY_INTERVAL_SECS: u64 = 60;

struct SysfsFailureState {
    battery_failed: bool,
    thermal_failed: bool,
    load_avg_failed: bool,
}

impl SysfsFailureState {
    fn new() -> Self {
        Self {
            battery_failed: false,
            thermal_failed: false,
            load_avg_failed: false,
        }
    }

    /// Log first failure, suppress repeats, log recovery.
    fn report_battery(&mut self, ok: bool) {
        if ok && self.battery_failed {
            eprintln!("Battery sysfs recovered");
            self.battery_failed = false;
        } else if !ok && !self.battery_failed {
            eprintln!("Warning: battery sysfs read failed, showing '--'");
            self.battery_failed = true;
        }
    }

    fn report_thermal(&mut self, reading: &str) {
        let ok = reading != "--";
        if ok && self.thermal_failed {
            eprintln!("Thermal sysfs recovered");
            self.thermal_failed = false;
        } else if !ok && !self.thermal_failed {
            eprintln!("Warning: thermal sysfs read failed, showing '--'");
            self.thermal_failed = true;
        }
    }

    fn report_load_avg(&mut self, reading: &str) {
        let ok = reading != "--";
        if ok && self.load_avg_failed {
            eprintln!("Load average recovered");
            self.load_avg_failed = false;
        } else if !ok && !self.load_avg_failed {
            eprintln!("Warning: /proc/loadavg read failed, showing '--'");
            self.load_avg_failed = true;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BatteryState {
    NotCharging,
    Charging,
    Low,
}

enum ButtonImage {
    Text(String),
    Icon(Option<String>),
    Time(Vec<ChronoItem<'static>>, Locale),
    Battery(String),
    Memory,
    LoadAvg,
    Temperature(String),
    Spacer,
    Workspaces,
    WindowTitle,
    Volume,
}

struct Button {
    image: ButtonImage,
    changed: bool,
    active: bool,
    action: Vec<Key>,
    color: Option<(f64, f64, f64)>,
}

fn find_battery_device() -> Option<String> {
    let power_supply_path = "/sys/class/power_supply";
    if let Ok(entries) = fs::read_dir(power_supply_path) {
        for entry in entries.flatten() {
            let dev_path = entry.path();
            let type_path = dev_path.join("type");
            if let Ok(typ) = fs::read_to_string(&type_path) {
                if typ.trim() == "Battery" {
                    if let Some(name) = dev_path.file_name().and_then(|n| n.to_str()) {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    None
}

fn get_battery_state(battery: &str) -> Option<(u32, BatteryState)> {
    let status_path = format!("/sys/class/power_supply/{}/status", battery);
    let status = fs::read_to_string(&status_path)
        .unwrap_or_else(|_| "Unknown".to_string());

    let capacity = {
        let base = format!("/sys/class/power_supply/{}", battery);
        let from_ratio = |num_file: &str, den_file: &str| -> Option<u32> {
            let num = fs::read_to_string(format!("{}/{}", base, num_file))
                .ok()?.trim().parse::<f64>().ok()?;
            let den = fs::read_to_string(format!("{}/{}", base, den_file))
                .ok()?.trim().parse::<f64>().ok().filter(|v| *v > 0.0)?;
            Some(((num / den) * 100.0).round() as u32)
        };
        from_ratio("charge_now", "charge_full")
            .or_else(|| from_ratio("energy_now", "energy_full"))
            .or_else(|| {
                fs::read_to_string(format!("{}/capacity", base))
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
            })?
    };

    let state = match status.trim() {
        "Charging" | "Full" => BatteryState::Charging,
        "Discharging" if capacity < 10 => BatteryState::Low,
        _ => BatteryState::NotCharging,
    };
    Some((capacity, state))
}

/// Returns estimated time remaining as a formatted string (e.g. "7:51" or "1:23 to full").
/// Returns None if estimation is not possible.
fn get_battery_time_estimate(battery: &str, charging: bool) -> Option<String> {
    let base = format!("/sys/class/power_supply/{}", battery);
    let read_val = |file: &str| -> Option<f64> {
        fs::read_to_string(format!("{}/{}", base, file))
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| *v != 0.0)
    };

    // 1. Try direct time_to_* files (value in seconds)
    let time_secs = if charging {
        read_val("time_to_full_now")
    } else {
        read_val("time_to_empty_now")
    };
    if let Some(secs) = time_secs {
        let h = (secs / 3600.0) as u32;
        let m = ((secs % 3600.0) / 60.0) as u32;
        return Some(format!("{}h{:02}m", h, m));
    }

    // 2. Try energy_now / power_now
    let energy_now = read_val("energy_now");
    let energy_full = read_val("energy_full");
    let power_now = read_val("power_now").map(|v| v.abs());

    // 3. Derive energy from charge × voltage if needed
    let (energy_now, energy_full) = match (energy_now, energy_full) {
        (Some(en), Some(ef)) => (Some(en), Some(ef)),
        _ => {
            let voltage = read_val("voltage_now")?;
            let cn = read_val("charge_now")?;
            let cf = read_val("charge_full")?;
            (
                Some(cn * voltage / 1_000_000.0),
                Some(cf * voltage / 1_000_000.0),
            )
        }
    };

    // 4. Derive power from current × voltage if needed
    let power = power_now.or_else(|| {
        let voltage = read_val("voltage_now")?;
        let current = read_val("current_now").map(|v| v.abs())?;
        Some(voltage * current / 1_000_000.0)
    });

    let (en, ef, pw) = (energy_now?, energy_full?, power.filter(|v| *v > 0.0)?);
    let hours = if charging { (ef - en) / pw } else { en / pw };
    let h = hours as u32;
    let m = ((hours - h as f64) * 60.0) as u32;
    Some(format!("{}h{:02}m", h, m))
}

fn get_load_avg() -> String {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(|v| v.to_string()))
        .unwrap_or_else(|| "--".to_string())
}

fn find_thermal_zone() -> Option<String> {
    let base = "/sys/class/thermal";
    // Prefer CPU-related zones, fall back to first available
    let mut best: Option<(String, i32)> = None;
    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("thermal_zone") {
                continue;
            }
            let type_str = fs::read_to_string(entry.path().join("type"))
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            let priority = if type_str.contains("cpu") || type_str.contains("soc") {
                2
            } else if type_str.contains("battery") || type_str.contains("gpu") {
                0
            } else {
                1
            };
            if best.as_ref().map_or(true, |(_, p)| priority > *p) {
                best = Some((name, priority));
            }
        }
    }
    best.map(|(name, _)| name)
}

fn get_temperature(zone: &str) -> String {
    let path = format!("/sys/class/thermal/{}/temp", zone);
    match fs::read_to_string(&path) {
        Ok(s) => match s.trim().parse::<f64>() {
            Ok(millideg) => format!("{:.0}°C", millideg / 1000.0),
            Err(_) => "--".to_string(),
        },
        Err(_) => "--".to_string(),
    }
}


fn resolve_icon_path(name: &str) -> Option<String> {
    let candidates = [
        format!("/etc/tiny-dfr/{name}.svg"),
        format!("/usr/share/tiny-dfr/{name}.svg"),
    ];
    for path in &candidates {
        if Path::new(path).exists() {
            return Some(path.clone());
        }
    }
    eprintln!("Warning: icon '{name}' not found");
    None
}

impl Button {
    fn with_config(cfg: ButtonConfig) -> Button {
        let color = cfg.color;
        let mut button = if let Some(text) = cfg.text {
            Button::new_text(text, cfg.action)
        } else if let Some(icon) = cfg.icon {
            Button::new_icon(&icon, cfg.action)
        } else if let Some(time) = cfg.time {
            Button::new_time(cfg.action, &time, cfg.locale.as_deref())
        } else if let Some(battery_mode) = cfg.battery {
            if let Some(battery) = find_battery_device() {
                Button::new_battery(cfg.action, battery, battery_mode)
            } else {
                Button::new_text("Battery N/A".to_string(), cfg.action)
            }
        } else if cfg.memory == Some(true) {
            Button {
                action: vec![],
                active: false,
                changed: false,
                image: ButtonImage::Memory,
                color: None,
            }
        } else if cfg.load_avg == Some(true) {
            Button {
                action: vec![],
                active: false,
                changed: false,
                image: ButtonImage::LoadAvg,
                color: None,
            }
        } else if cfg.temperature == Some(true) {
            if let Some(zone) = find_thermal_zone() {
                Button {
                    action: vec![],
                    active: false,
                    changed: false,
                    image: ButtonImage::Temperature(zone),
                    color: None,
                }
            } else {
                Button::new_text("Temp N/A".to_string(), vec![])
            }
        } else if cfg.workspaces == Some(true) {
            Button {
                action: vec![],
                active: false,
                changed: false,
                image: ButtonImage::Workspaces,
                color: None,
            }
        } else if cfg.window_title == Some(true) {
            Button {
                action: vec![],
                active: false,
                changed: false,
                image: ButtonImage::WindowTitle,
                color: None,
            }
        } else if cfg.volume == Some(true) {
            Button {
                action: vec![],
                active: false,
                changed: false,
                image: ButtonImage::Volume,
                color: None,
            }
        } else {
            Button::new_spacer()
        };
        button.color = color;
        button
    }
    fn new_spacer() -> Button {
        Button {
            action: vec![],
            active: false,
            changed: false,
            image: ButtonImage::Spacer,
            color: None,
        }
    }
    fn new_text(text: String, action: Vec<Key>) -> Button {
        Button {
            action,
            active: false,
            changed: false,
            image: ButtonImage::Text(text),
            color: None,
        }
    }
    fn new_icon(name: impl AsRef<str>, action: Vec<Key>) -> Button {
        let path = resolve_icon_path(name.as_ref());
        Button {
            action,
            image: ButtonImage::Icon(path),
            active: false,
            changed: false,
            color: None,
        }
    }
    fn new_battery(action: Vec<Key>, battery: String, _battery_mode: String) -> Button {
        Button {
            action,
            active: false,
            changed: false,
            image: ButtonImage::Battery(battery),
            color: None,
        }
    }

    fn new_time(action: Vec<Key>, format: &str, locale_str: Option<&str>) -> Button {
        let format_str = if format == "24hr" {
            "%H:%M    %a %-e %b"
        } else if format == "12hr" {
            "%-l:%M %p    %a %-e %b"
        } else {
            format
        };

        let format_items = match StrftimeItems::new(format_str).parse_to_owned() {
            Ok(s) => s,
            Err(e) => panic!("Invalid time format, consult the configuration file for examples of correct ones: {e:?}"),
        };

        let locale = locale_str.and_then(|l| Locale::try_from(l).ok()).unwrap_or(Locale::POSIX);
        Button {
            action,
            active: false,
            changed: false,
            image: ButtonImage::Time(format_items, locale),
            color: None,
        }
    }
    fn needs_faster_refresh(&self) -> bool {
        match &self.image {
            ButtonImage::Time(items, _) =>
                items.iter().any(|item| {
                    use chrono::format::{Item, Numeric};
                    match item {
                        Item::Numeric(Numeric::Second, _) |
                        Item::Numeric(Numeric::Nanosecond, _) |
                        Item::Numeric(Numeric::Timestamp, _) => true,
                        _ => false,
                    }
                }),
            _ => false,
        }
    }
    fn set_active<F>(&mut self, uinput: &mut UInputHandle<F>, active: bool)
    where
        F: AsRawFd,
    {
        if self.active != active {
            self.active = active;
            self.changed = true;

            toggle_keys(uinput, &self.action, active as i32);
        }
    }
}

#[derive(Default)]
pub struct FunctionLayer {
    displays_time: bool,
    displays_battery: bool,
    displays_memory: bool,
    displays_load_avg: bool,
    displays_temperature: bool,
    /// Each entry is (width_fraction, Button) where width_fraction is 0.0–1.0
    buttons: Vec<(f64, Button)>,
    faster_refresh: bool,
    memory_sample_interval_ms: u32,
    memory_graph_window_s: u32,
}

impl FunctionLayer {
    fn with_config(cfg: Vec<ButtonConfig>) -> FunctionLayer {
        if cfg.is_empty() {
            panic!("Invalid configuration, layer has 0 buttons");
        }

        let displays_time = cfg.iter().any(|cfg| cfg.time.is_some());
        let displays_battery = cfg.iter().any(|cfg| cfg.battery.is_some());
        let displays_memory = cfg.iter().any(|cfg| cfg.memory == Some(true));
        let displays_load_avg = cfg.iter().any(|cfg| cfg.load_avg == Some(true));
        let displays_temperature = cfg.iter().any(|cfg| cfg.temperature == Some(true));

        // Extract graph config from first Memory button
        let mem_btn = cfg.iter().find(|c| c.memory == Some(true));
        let memory_sample_interval_ms = mem_btn.and_then(|c| c.sample_interval).unwrap_or(1000);
        let memory_graph_window_s = mem_btn.and_then(|c| c.graph_window).unwrap_or(60);

        // Compute width fractions. Buttons with an explicit Width (percentage)
        // get that share; the rest split whatever remains equally.
        let specified_total: f64 = cfg.iter().filter_map(|c| c.width).sum();
        let unspecified_count = cfg.iter().filter(|c| c.width.is_none()).count();
        let remaining = (100.0 - specified_total).max(0.0);
        let default_width = if unspecified_count > 0 {
            remaining / unspecified_count as f64
        } else {
            0.0
        };

        let buttons: Vec<(f64, Button)> = cfg
            .into_iter()
            .map(|c| {
                let frac = c.width.unwrap_or(default_width) / 100.0;
                (frac, Button::with_config(c))
            })
            .collect();
        let faster_refresh = buttons.iter().any(|(_, b)| b.needs_faster_refresh());
        FunctionLayer {
            displays_time,
            displays_battery,
            displays_memory,
            displays_load_avg,
            displays_temperature,
            buttons,
            faster_refresh,
            memory_sample_interval_ms,
            memory_graph_window_s,
        }
    }
}

struct Interface;

impl LibinputInterface for Interface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        let mode = flags & O_ACCMODE;

        OpenOptions::new()
            .custom_flags(flags)
            .read(mode == O_RDONLY || mode == O_RDWR)
            .write(mode == O_WRONLY || mode == O_RDWR)
            .open(path)
            .map(|file| file.into())
            .map_err(|err| err.raw_os_error().unwrap())
    }
    fn close_restricted(&mut self, fd: OwnedFd) {
        _ = File::from(fd);
    }
}

fn emit<F>(uinput: &mut UInputHandle<F>, ty: EventKind, code: u16, value: i32)
where
    F: AsRawFd,
{
    uinput
        .write(&[input_event {
            value,
            type_: ty as u16,
            code,
            time: timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
        }])
        .unwrap();
}

fn toggle_keys<F>(uinput: &mut UInputHandle<F>, codes: &Vec<Key>, value: i32)
where
    F: AsRawFd,
{
    if codes.is_empty() {
        return;
    }
    for kc in codes {
        emit(uinput, EventKind::Key, *kc as u16, value);
    }
    emit(
        uinput,
        EventKind::Synchronize,
        SynchronizeKind::Report as u16,
        0,
    );
}

fn build_button_defs(
    layer: &FunctionLayer,
    ws: Option<(&WorkspaceManager, &WorkspacesConfig)>,
    volume_mgr: Option<&VolumeManager>,
    memory_history: Option<&MemoryHistory>,
    blink_on: bool,
    show_battery_time: bool,
    sysfs_state: &mut SysfsFailureState,
) -> Vec<ButtonDef> {
    let mut defs = Vec::new();
    for (idx, (width_frac, b)) in layer.buttons.iter().enumerate() {
        match &b.image {
            ButtonImage::Workspaces => {
                let workspaces = ws
                    .map(|(mgr, _)| mgr.workspaces())
                    .unwrap_or_default();
                if workspaces.is_empty() {
                    continue;
                }
                let ws_cfg = ws.map(|(_, c)| c);
                let per_ws = width_frac / workspaces.len() as f64;
                for w in &workspaces {
                    let color = if w.is_urgent {
                        ws_cfg.map(|c| c.urgent_color)
                    } else if w.is_focused {
                        ws_cfg.map(|c| c.active_color)
                    } else {
                        None
                    };
                    defs.push(ButtonDef {
                        label: w.name.clone().unwrap_or_else(|| w.idx.to_string()),
                        active: w.is_focused,
                        width_fraction: per_ws,
                        color,
                        action: ButtonAction::Workspace(w.id),
                        graph_data: None,
                        graph_max_columns: None,
                        battery: None,
                        icon: None,
                    });
                }
            }
            ButtonImage::WindowTitle => {
                let title = ws
                    .and_then(|(mgr, _)| mgr.focused_window_title())
                    .unwrap_or_default();
                defs.push(ButtonDef {
                    label: title,
                    active: false,
                    width_fraction: *width_frac,
                    color: b.color,
                    action: ButtonAction::None,
                    graph_data: None,
                    graph_max_columns: None,
                    battery: None,
                    icon: None,
                });
            }
            ButtonImage::Spacer => {
                defs.push(ButtonDef {
                    label: String::new(),
                    active: false,
                    width_fraction: *width_frac,
                    color: b.color,
                    action: ButtonAction::None,
                    graph_data: None,
                    graph_max_columns: None,
                    battery: None,
                    icon: None,
                });
            }
            ButtonImage::Memory => {
                if let Some(history) = memory_history {
                    defs.push(ButtonDef {
                        label: String::new(),
                        active: b.active,
                        width_fraction: *width_frac,
                        color: b.color,
                        action: ButtonAction::None,
                        graph_data: Some(history.samples().iter().copied().collect()),
                        graph_max_columns: Some(history.max_samples()),
                        battery: None,
                        icon: None,
                    });
                } else {
                    defs.push(ButtonDef {
                        label: format!("{}%", memory_graph::get_memory_usage()),
                        active: b.active,
                        width_fraction: *width_frac,
                        color: b.color,
                        action: ButtonAction::None,
                        graph_data: None,
                        graph_max_columns: None,
                        battery: None,
                        icon: None,
                    });
                }
            }
            ButtonImage::Battery(battery) => {
                let battery_data = get_battery_state(battery);
                sysfs_state.report_battery(battery_data.is_some());
                let (label, battery_info) = match battery_data {
                    Some((capacity, state)) => {
                        let charging = state == BatteryState::Charging;
                        let show = if capacity < 10 && !charging { blink_on } else { true };
                        (
                            format!("{capacity}%"),
                            BatteryInfo {
                                capacity,
                                charging,
                                blink_on: show,
                                time_estimate: if show_battery_time {
                                    get_battery_time_estimate(battery, charging)
                                } else {
                                    None
                                },
                                show_time: show_battery_time,
                            },
                        )
                    }
                    None => (
                        "--".to_string(),
                        BatteryInfo {
                            capacity: 0,
                            charging: false,
                            blink_on: true,
                            time_estimate: None,
                            show_time: false,
                        },
                    ),
                };
                defs.push(ButtonDef {
                    label,
                    active: b.active,
                    width_fraction: *width_frac,
                    color: b.color,
                    action: ButtonAction::LayerButton(idx),
                    graph_data: None,
                    graph_max_columns: None,
                    battery: Some(battery_info),
                    icon: None,
                });
            }
            ButtonImage::Volume => {
                let label = if let Some(mgr) = volume_mgr {
                    let vol = mgr.volume();
                    if vol.muted {
                        "muted".to_string()
                    } else {
                        format!("{}%", vol.volume_percent)
                    }
                } else {
                    "Vol N/A".to_string()
                };
                defs.push(ButtonDef {
                    label,
                    active: b.active,
                    width_fraction: *width_frac,
                    color: b.color,
                    action: ButtonAction::Volume {
                        down_icon: resolve_icon_path("volume_down"),
                        up_icon: resolve_icon_path("volume_up"),
                    },
                    graph_data: None,
                    graph_max_columns: None,
                    battery: None,
                    icon: None,
                });
            }
            ButtonImage::Icon(ref path) => {
                defs.push(ButtonDef {
                    label: String::new(),
                    active: b.active,
                    width_fraction: *width_frac,
                    color: b.color,
                    action: ButtonAction::LayerButton(idx),
                    graph_data: None,
                    graph_max_columns: None,
                    battery: None,
                    icon: path.clone(),
                });
            }
            _ => {
                defs.push(ButtonDef {
                    label: match &b.image {
                        ButtonImage::Text(s) => s.clone(),
                        ButtonImage::Time(format, locale) => {
                            Local::now()
                                .format_localized_with_items(format.iter(), *locale)
                                .to_string()
                        }
                        ButtonImage::LoadAvg => {
                            let val = get_load_avg();
                            sysfs_state.report_load_avg(&val);
                            val
                        }
                        ButtonImage::Temperature(zone) => {
                            let val = get_temperature(zone);
                            sysfs_state.report_thermal(&val);
                            val
                        }
                        _ => unreachable!(),
                    },
                    active: b.active,
                    width_fraction: *width_frac,
                    color: b.color,
                    action: ButtonAction::LayerButton(idx),
                    graph_data: None,
                    graph_max_columns: None,
                    battery: None,
                    icon: None,
                });
            }
        }
    }
    defs
}

fn main() {
    let mut drm = DrmBackend::open_card().unwrap();
    let (height, width) = drm.mode().size();
    let _ = panic::catch_unwind(AssertUnwindSafe(|| real_main(&mut drm)));
    let crash_bitmap = include_bytes!("crash_bitmap.raw");
    let mut map = drm.map().unwrap();
    let data = map.as_mut();
    let mut wptr = 0;
    for byte in crash_bitmap {
        for i in 0..8 {
            let bit = ((byte >> i) & 0x1) == 0;
            let color = if bit { 0xFF } else { 0x0 };
            data[wptr] = color;
            data[wptr + 1] = color;
            data[wptr + 2] = color;
            data[wptr + 3] = color;
            wptr += 4;
        }
    }
    drop(map);
    drm.dirty(&[ClipRect::new(0, 0, height, width)]).unwrap();
    let mut sigset = SigSet::empty();
    sigset.add(Signal::SIGTERM);
    sigset.wait().unwrap();
}

fn real_main(drm: &mut DrmBackend) {
    let (height, width) = drm.mode().size();
    let (db_width, db_height) = drm.fb_info().unwrap().size();
    let mut uinput = UInputHandle::new(OpenOptions::new().write(true).open("/dev/uinput").unwrap());
    let mut backlight = BacklightManager::new();
    let mut cfg_mgr = ConfigManager::new();
    let (mut cfg, mut layers) = cfg_mgr.load_config(width)
        .expect("Failed to load initial configuration");
    let mut pixel_shift = PixelShiftManager::new();

    // Detect graphical session user before dropping privileges
    let session_user = session_detect::detect_graphical_session_user();

    // Drop privileges to detected session user (or nobody as fallback)
    let drop_username = match &session_user {
        Some(u) => {
            eprintln!("Detected graphical session user: {} (uid={})", u.username, u.uid);
            u.username.as_str()
        }
        None => {
            eprintln!("Warning: no graphical session found, falling back to nobody (niri/pulse won't work)");
            "nobody"
        }
    };
    PrivDrop::default()
        .user(drop_username)
        .group_list(&["input", "video"])
        .include_default_supplementary_groups()
        .apply()
        .unwrap_or_else(|e| panic!("Failed to drop privileges: {}", e));

    // Set environment variables for the target user's session
    if let Some(ref user) = session_user {
        let xdg_runtime = format!("/run/user/{}", user.uid);
        std::env::set_var("XDG_RUNTIME_DIR", &xdg_runtime);
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/bus", xdg_runtime));

        if let Some(niri_socket) = session_detect::discover_niri_socket(user.uid) {
            eprintln!("Discovered NIRI_SOCKET: {}", niri_socket);
            std::env::set_var("NIRI_SOCKET", &niri_socket);
        } else {
            eprintln!("Warning: could not discover NIRI_SOCKET (niri may not be running)");
        }
    }

    // Create workspace manager (always-on, starts disconnected) if configured
    let workspace_mgr = cfg.workspaces.as_ref().map(|ws_cfg| {
        eprintln!("Workspaces config present, provider={:?}", ws_cfg.provider);
        let mgr = WorkspaceManager::new(ws_cfg.provider.as_deref());
        if !mgr.try_connect() {
            eprintln!("Warning: [Workspaces] configured but could not connect (will reconnect when available)");
        }
        mgr
    });

    // Create volume manager (always-on, starts disconnected) if configured
    let volume_mgr = cfg.volume.as_ref().map(|vol_cfg| {
        eprintln!("Volume config present, pulse_server={:?}", vol_cfg.pulse_server);
        let mgr = VolumeManager::new(vol_cfg.pulse_server.as_deref());
        if !mgr.try_connect() {
            eprintln!("Warning: [Volume] configured but could not connect (will reconnect when available)");
        }
        mgr
    });

    let mut iced_rndr = TouchbarRenderer::new(
        width as u32, height as u32, db_width as u32,
        &cfg.font_family, cfg.font_size,
        cfg.font_bold, cfg.font_italic,
    );
    let mut active_layer = 0;
    let mut needs_complete_redraw = true;
    let mut blink_on = true;
    let mut last_blink_toggle = std::time::Instant::now();
    let mut battery_show_time_until: Option<std::time::Instant> = None;

    let mut sysfs_state = SysfsFailureState::new();
    let mut last_sysfs_retry = Instant::now();

    let mut memory_history = if layers.iter().any(|l| l.displays_memory) {
        Some(MemoryHistory::new(
            layers[0].memory_sample_interval_ms,
            layers[0].memory_graph_window_s,
        ))
    } else {
        None
    };

    let mut input_tb = Libinput::new_with_udev(Interface);
    let mut input_main = Libinput::new_with_udev(Interface);
    input_tb.udev_assign_seat("seat-touchbar").unwrap();
    input_main.udev_assign_seat("seat0").unwrap();
    let udev_monitor = MonitorBuilder::new()
        .unwrap()
        .match_subsystem("power_supply")
        .unwrap()
        .listen()
        .unwrap();
    let epoll = Epoll::new(EpollCreateFlags::empty()).unwrap();
    epoll
        .add(input_main.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 0))
        .unwrap();
    epoll
        .add(input_tb.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 1))
        .unwrap();
    epoll
        .add(cfg_mgr.fd(), EpollEvent::new(EpollFlags::EPOLLIN, 2))
        .unwrap();
    epoll
        .add(&udev_monitor, EpollEvent::new(EpollFlags::EPOLLIN, 3))
        .unwrap();
    if let Some(ref mgr) = workspace_mgr {
        epoll
            .add(mgr.event_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 4))
            .unwrap();
    }
    if let Some(ref mgr) = volume_mgr {
        epoll
            .add(mgr.event_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 5))
            .unwrap();
    }

    // Create reconnect watcher for socket directories (inotify-based)
    let xdg_runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }));
    let mut reconnect_watcher = ReconnectWatcher::new(&xdg_runtime_dir);
    epoll
        .add(reconnect_watcher.fd(), EpollEvent::new(EpollFlags::EPOLLIN, 6))
        .unwrap();
    // Cooldown for poll-based reconnection attempts (avoid busy-looping on try_connect)
    let mut last_reconnect_attempt: Option<Instant> = None;
    const RECONNECT_COOLDOWN_SECS: u64 = 2;

    uinput.set_evbit(EventKind::Key).unwrap();
    for layer in &layers {
        for button in &layer.buttons {
            for k in &button.1.action {
                uinput.set_keybit(*k).unwrap();
            }
        }
    }
    uinput.set_keybit(Key::VolumeDown).unwrap();
    uinput.set_keybit(Key::VolumeUp).unwrap();
    let mut dev_name_c = [0 as c_char; 80];
    let dev_name = "Dynamic Function Row Virtual Input Device".as_bytes();
    for i in 0..dev_name.len() {
        dev_name_c[i] = dev_name[i] as c_char;
    }
    uinput
        .dev_setup(&uinput_setup {
            id: input_id {
                bustype: 0x19,
                vendor: 0x1209,
                product: 0x316E,
                version: 1,
            },
            ff_effects_max: 0,
            name: dev_name_c,
        })
        .unwrap();
    uinput.dev_create().unwrap();

    let mut digitizer: Option<InputDevice> = None;
    let mut touch_positions: HashMap<u32, iced_core::Point> = HashMap::new();
    let mut last_redraw_ts = if layers[active_layer].faster_refresh {
        Local::now().second()
    } else {
        Local::now().minute()
    };
    loop {
        if cfg_mgr.update_config(&mut cfg, &mut layers, width) {
            active_layer = 0;
            needs_complete_redraw = true;
            iced_rndr = TouchbarRenderer::new(
                width as u32, height as u32, db_width as u32,
                &cfg.font_family, cfg.font_size,
                cfg.font_bold, cfg.font_italic,
            );
            memory_history = if layers.iter().any(|l| l.displays_memory) {
                Some(MemoryHistory::new(
                    layers[0].memory_sample_interval_ms,
                    layers[0].memory_graph_window_s,
                ))
            } else {
                None
            };
        }

        let now = Local::now();
        let ms_left = ((60 - now.second()) * 1000) as i32;
        let mut next_timeout_ms = min(ms_left, TIMEOUT_MS);

        if cfg.enable_pixel_shift {
            let (pixel_shift_needs_redraw, pixel_shift_next_timeout_ms) = pixel_shift.update();
            if pixel_shift_needs_redraw {
                needs_complete_redraw = true;
            }
            next_timeout_ms = min(next_timeout_ms, pixel_shift_next_timeout_ms);
        }

        let current_ts = if layers[active_layer].faster_refresh {
            Local::now().second()
        } else {
            Local::now().minute()
        };
        if layers[active_layer].displays_time && (current_ts != last_redraw_ts) {
            needs_complete_redraw = true;
            last_redraw_ts = current_ts;
        }
        if layers[active_layer].displays_battery {
            let now_instant = std::time::Instant::now();
            if now_instant.duration_since(last_blink_toggle).as_millis() >= 500 {
                blink_on = !blink_on;
                last_blink_toggle = now_instant;
            }
            let elapsed = now_instant.duration_since(last_blink_toggle).as_millis() as i32;
            next_timeout_ms = min(next_timeout_ms, (500 - elapsed).max(50) as i32);

            for button in &mut layers[active_layer].buttons {
                if let ButtonImage::Battery(_) = button.1.image {
                    button.1.changed = true;
                }
            }
        }
        if let Some(deadline) = battery_show_time_until {
            let now_instant = std::time::Instant::now();
            if now_instant >= deadline {
                battery_show_time_until = None;
                needs_complete_redraw = true;
            } else {
                let remaining = deadline.duration_since(now_instant).as_millis() as i32;
                next_timeout_ms = min(next_timeout_ms, remaining.max(50));
            }
        }
        if let Some(ref mut history) = memory_history {
            if history.maybe_sample() {
                needs_complete_redraw = true;
            }
            next_timeout_ms = min(next_timeout_ms, history.sample_interval_ms() as i32);
        }
        if layers[active_layer].displays_load_avg {
            for button in &mut layers[active_layer].buttons {
                if let ButtonImage::LoadAvg = button.1.image {
                    button.1.changed = true;
                }
            }
        }
        if layers[active_layer].displays_temperature {
            for button in &mut layers[active_layer].buttons {
                if let ButtonImage::Temperature(_) = button.1.image {
                    button.1.changed = true;
                }
            }
        }

        // Check for reconnection flashes -- trigger redraw to show flash effect
        let ws_flash = workspace_mgr.as_ref().map_or(false, |m| m.has_reconnect_flash());
        let vol_flash = volume_mgr.as_ref().map_or(false, |m| m.has_reconnect_flash());
        if ws_flash || vol_flash {
            needs_complete_redraw = true;
        }

        if needs_complete_redraw || layers[active_layer].buttons.iter().any(|b| b.1.changed) {
            // Only pass workspace/volume managers when connected (hide widgets when disconnected)
            let ws = workspace_mgr.as_ref()
                .filter(|mgr| mgr.is_connected())
                .zip(cfg.workspaces.as_ref());
            let vol = volume_mgr.as_ref()
                .filter(|mgr| mgr.is_connected());
            let btn_defs = build_button_defs(&layers[active_layer], ws, vol, memory_history.as_ref(), blink_on, battery_show_time_until.is_some(), &mut sysfs_state);
            let buffer = iced_rndr.render_to_buffer(&btn_defs);
            drm.map().unwrap().as_mut()[..buffer.len()].copy_from_slice(&buffer);
            drm.dirty(&[ClipRect::new(0, 0, height, width)]).unwrap();
            for (_, btn) in &mut layers[active_layer].buttons {
                btn.changed = false;
            }
            needs_complete_redraw = false;
        }

        match epoll.wait(
            &mut [EpollEvent::new(EpollFlags::EPOLLIN, 0)],
            next_timeout_ms as u16,
        ) {
            Err(Errno::EINTR) | Ok(_) => 0,
            e => e.unwrap(),
        };

        _ = udev_monitor.iter().last();

        // Poll managers first to pick up disconnect events from their threads
        if let Some(ref mgr) = workspace_mgr {
            if mgr.poll() {
                needs_complete_redraw = true;
            }
        }
        if let Some(ref mgr) = volume_mgr {
            if mgr.poll() {
                needs_complete_redraw = true;
            }
        }

        // Check inotify for socket appearances and dispatch reconnection
        let reconnect_events = reconnect_watcher.check_events();
        // Restore any invalidated watches (e.g. after directory deletion/recreation)
        reconnect_watcher.ensure_watches();

        if reconnect_events.niri {
            if let Some(ref mgr) = workspace_mgr {
                if !mgr.is_connected() {
                    if mgr.try_connect() {
                        eprintln!("niri workspace: reconnected via inotify");
                        needs_complete_redraw = true;
                    }
                }
            }
        }
        if reconnect_events.pulse {
            if let Some(ref mgr) = volume_mgr {
                if !mgr.is_connected() {
                    if mgr.try_connect() {
                        eprintln!("PulseAudio volume: reconnected via inotify");
                        needs_complete_redraw = true;
                    }
                }
            }
        }

        // If a manager is disconnected but wasn't triggered by inotify,
        // try reconnecting anyway (handles races where socket was recreated
        // before we could re-add the inotify watch).
        // Throttled to avoid busy-looping on blocking try_connect() calls.
        let any_disconnected =
            workspace_mgr.as_ref().map_or(false, |m| !m.is_connected())
            || volume_mgr.as_ref().map_or(false, |m| !m.is_connected());
        let cooldown_elapsed = last_reconnect_attempt
            .map_or(true, |t| t.elapsed().as_secs() >= RECONNECT_COOLDOWN_SECS);
        if any_disconnected && cooldown_elapsed {
            last_reconnect_attempt = Some(Instant::now());
            if !reconnect_events.niri {
                if let Some(ref mgr) = workspace_mgr {
                    if !mgr.is_connected() {
                        if mgr.try_connect() {
                            eprintln!("niri workspace: reconnected via poll fallback");
                            needs_complete_redraw = true;
                        }
                    }
                }
            }
            if !reconnect_events.pulse {
                if let Some(ref mgr) = volume_mgr {
                    if !mgr.is_connected() {
                        if mgr.try_connect() {
                            eprintln!("PulseAudio volume: reconnected via poll fallback");
                            needs_complete_redraw = true;
                        }
                    }
                }
            }
        }

        // Periodic sysfs device retry (~60s) for disappeared devices
        if last_sysfs_retry.elapsed().as_secs() >= SYSFS_RETRY_INTERVAL_SECS {
            last_sysfs_retry = Instant::now();
            if sysfs_state.battery_failed {
                if let Some(new_battery) = find_battery_device() {
                    eprintln!("Sysfs retry: re-discovered battery device '{new_battery}'");
                    for layer in layers.iter_mut() {
                        for (_, btn) in &mut layer.buttons {
                            if let ButtonImage::Battery(ref mut name) = btn.image {
                                *name = new_battery.clone();
                            }
                        }
                    }
                    needs_complete_redraw = true;
                }
            }
            if sysfs_state.thermal_failed {
                if let Some(new_zone) = find_thermal_zone() {
                    eprintln!("Sysfs retry: re-discovered thermal zone '{new_zone}'");
                    for layer in layers.iter_mut() {
                        for (_, btn) in &mut layer.buttons {
                            if let ButtonImage::Temperature(ref mut zone) = btn.image {
                                *zone = new_zone.clone();
                            }
                        }
                    }
                    needs_complete_redraw = true;
                }
            }
        }

        if let Err(e) = input_tb.dispatch() {
            eprintln!("Warning: touchbar input dispatch error: {e}");
        }
        if let Err(e) = input_main.dispatch() {
            eprintln!("Warning: main input dispatch error: {e}");
        }
        for event in &mut input_tb.clone().chain(input_main.clone()) {
            backlight.process_event(&event);
            if backlight.take_lid_opened() {
                needs_complete_redraw = true;
            }
            match event {
                Event::Device(DeviceEvent::Added(evt)) => {
                    let dev = evt.device();
                    if dev.name().contains(" Touch Bar") {
                        digitizer = Some(dev);
                    }
                }
                Event::Keyboard(KeyboardEvent::Key(key)) => {
                    if key.key() == Key::Fn as u32 {
                        let new_layer = match key.key_state() {
                            KeyState::Pressed => 1,
                            KeyState::Released => 0,
                        };
                        if active_layer != new_layer {
                            active_layer = new_layer;
                            needs_complete_redraw = true;
                        }
                    }
                }
                Event::Touch(te) => {
                    if Some(te.device()) != digitizer || backlight.current_bl() == 0 {
                        continue;
                    }
                    let (iced_event, cursor) = match &te {
                        TouchEvent::Down(dn) => {
                            let pos = iced_core::Point::new(
                                dn.x_transformed(width as u32) as f32,
                                dn.y_transformed(height as u32) as f32,
                            );
                            touch_positions.insert(dn.seat_slot(), pos);
                            (
                                iced_core::Event::Touch(iced_core::touch::Event::FingerPressed {
                                    id: iced_core::touch::Finger(dn.seat_slot() as u64),
                                    position: pos,
                                }),
                                iced_core::mouse::Cursor::Available(pos),
                            )
                        }
                        TouchEvent::Motion(mv) => {
                            let pos = iced_core::Point::new(
                                mv.x_transformed(width as u32) as f32,
                                mv.y_transformed(height as u32) as f32,
                            );
                            touch_positions.insert(mv.seat_slot(), pos);
                            (
                                iced_core::Event::Touch(iced_core::touch::Event::FingerMoved {
                                    id: iced_core::touch::Finger(mv.seat_slot() as u64),
                                    position: pos,
                                }),
                                iced_core::mouse::Cursor::Available(pos),
                            )
                        }
                        TouchEvent::Up(up) => {
                            let pos = touch_positions
                                .remove(&up.seat_slot())
                                .unwrap_or(iced_core::Point::ORIGIN);
                            (
                                iced_core::Event::Touch(iced_core::touch::Event::FingerLifted {
                                    id: iced_core::touch::Finger(up.seat_slot() as u64),
                                    position: pos,
                                }),
                                iced_core::mouse::Cursor::Available(pos),
                            )
                        }
                        _ => continue,
                    };

                    let ws = workspace_mgr.as_ref()
                        .filter(|mgr| mgr.is_connected())
                        .zip(cfg.workspaces.as_ref());
                    let vol = volume_mgr.as_ref()
                        .filter(|mgr| mgr.is_connected());
                    let btn_defs = build_button_defs(&layers[active_layer], ws, vol, memory_history.as_ref(), blink_on, battery_show_time_until.is_some(), &mut sysfs_state);
                    let messages = iced_rndr.process_touch(iced_event, cursor, &btn_defs);

                    for msg in messages {
                        match msg {
                            IcedMessage::ButtonDown(i) => {
                                if i < layers[active_layer].buttons.len() {
                                    layers[active_layer].buttons[i]
                                        .1
                                        .set_active(&mut uinput, true);
                                }
                            }
                            IcedMessage::ButtonUp(i) => {
                                if i < layers[active_layer].buttons.len() {
                                    layers[active_layer].buttons[i]
                                        .1
                                        .set_active(&mut uinput, false);
                                    if let ButtonImage::Battery(_) = layers[active_layer].buttons[i].1.image {
                                        battery_show_time_until = Some(
                                            std::time::Instant::now() + std::time::Duration::from_secs(2),
                                        );
                                        needs_complete_redraw = true;
                                    }
                                }
                            }
                            IcedMessage::WorkspaceDown(_) => {
                                needs_complete_redraw = true;
                            }
                            IcedMessage::WorkspaceUp(id) => {
                                if let Some(ref mgr) = workspace_mgr {
                                    mgr.focus_workspace(id);
                                }
                            }
                            IcedMessage::VolumeDownPress => {
                                toggle_keys(&mut uinput, &vec![Key::VolumeDown], 1);
                            }
                            IcedMessage::VolumeDownRelease => {
                                toggle_keys(&mut uinput, &vec![Key::VolumeDown], 0);
                                needs_complete_redraw = true;
                            }
                            IcedMessage::VolumeUpPress => {
                                toggle_keys(&mut uinput, &vec![Key::VolumeUp], 1);
                            }
                            IcedMessage::VolumeUpRelease => {
                                toggle_keys(&mut uinput, &vec![Key::VolumeUp], 0);
                                needs_complete_redraw = true;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        backlight.update_backlight(&cfg);
    }
}

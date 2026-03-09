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
};
use udev::MonitorBuilder;

mod backlight;
mod config;
mod display;
mod iced_renderer;
mod pixel_shift;

use crate::config::ConfigManager;
use backlight::BacklightManager;
use config::ButtonConfig;
use display::DrmBackend;
use iced_renderer::{ButtonDef, Message as IcedMessage, TouchbarRenderer};
use pixel_shift::PixelShiftManager;

const TIMEOUT_MS: i32 = 10 * 1000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BatteryState {
    NotCharging,
    Charging,
    Low,
}

enum ButtonImage {
    Text(String),
    Icon(String),
    Time(Vec<ChronoItem<'static>>, Locale),
    Battery(String),
    Spacer,
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

fn get_battery_state(battery: &str) -> (u32, BatteryState) {
    let status_path = format!("/sys/class/power_supply/{}/status", battery);
    let status = fs::read_to_string(&status_path)
        .unwrap_or_else(|_| "Unknown".to_string());

    let capacity = {
        #[cfg(target_arch = "x86_64")]
        {
            let charge_now_path = format!("/sys/class/power_supply/{}/charge_now", battery);
            let charge_full_path = format!("/sys/class/power_supply/{}/charge_full", battery);
            let charge_now = fs::read_to_string(&charge_now_path)
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok());
            let charge_full = fs::read_to_string(&charge_full_path)
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok());
            match (charge_now, charge_full) {
                (Some(now), Some(full)) if full > 0.0 => ((now / full) * 100.0).round() as u32,
                _ => 100,
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let capacity_path = format!("/sys/class/power_supply/{}/capacity", battery);
            fs::read_to_string(&capacity_path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(100)
        }
    };

    let status = match status.trim() {
        "Charging" | "Full" => BatteryState::Charging,
        "Discharging" if capacity < 10 => BatteryState::Low,
        _ => BatteryState::NotCharging,
    };
    (capacity, status)
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
        Button {
            action,
            image: ButtonImage::Icon(name.as_ref().to_string()),
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
    /// Each entry is (width_fraction, Button) where width_fraction is 0.0–1.0
    buttons: Vec<(f64, Button)>,
    faster_refresh: bool,
}

impl FunctionLayer {
    fn with_config(cfg: Vec<ButtonConfig>) -> FunctionLayer {
        if cfg.is_empty() {
            panic!("Invalid configuration, layer has 0 buttons");
        }

        let displays_time = cfg.iter().any(|cfg| cfg.time.is_some());
        let displays_battery = cfg.iter().any(|cfg| cfg.battery.is_some());

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
            buttons,
            faster_refresh,
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

fn build_button_defs(layer: &FunctionLayer) -> Vec<ButtonDef> {
    layer
        .buttons
        .iter()
        .map(|(width_frac, b)| ButtonDef {
            label: match &b.image {
                ButtonImage::Text(s) => s.clone(),
                ButtonImage::Time(format, locale) => {
                    Local::now()
                        .format_localized_with_items(format.iter(), *locale)
                        .to_string()
                }
                ButtonImage::Battery(battery) => {
                    let (capacity, _) = get_battery_state(battery);
                    format!("{capacity}%")
                }
                ButtonImage::Spacer => String::new(),
                ButtonImage::Icon(_) => "?".to_string(),
            },
            active: b.active,
            width_fraction: *width_frac,
            color: b.color,
        })
        .collect()
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
    let (mut cfg, mut layers) = cfg_mgr.load_config(width);
    let mut pixel_shift = PixelShiftManager::new();

    // drop privileges to input and video group
    let groups = ["input", "video"];

    PrivDrop::default()
        .user("nobody")
        .group_list(&groups)
        .apply()
        .unwrap_or_else(|e| panic!("Failed to drop privileges: {}", e));

    let mut iced_rndr = TouchbarRenderer::new(
        width as u32, height as u32, db_width as u32,
        &cfg.font_family, cfg.font_size,
        cfg.font_bold, cfg.font_italic,
    );
    let mut active_layer = 0;
    let mut needs_complete_redraw = true;

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
    uinput.set_evbit(EventKind::Key).unwrap();
    for layer in &layers {
        for button in &layer.buttons {
            for k in &button.1.action {
                uinput.set_keybit(*k).unwrap();
            }
        }
    }
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
            for button in &mut layers[active_layer].buttons {
                if let ButtonImage::Battery(_) = button.1.image {
                    button.1.changed = true;
                }
            }
        }

        if needs_complete_redraw || layers[active_layer].buttons.iter().any(|b| b.1.changed) {
            let btn_defs = build_button_defs(&layers[active_layer]);
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

        input_tb.dispatch().unwrap();
        input_main.dispatch().unwrap();
        for event in &mut input_tb.clone().chain(input_main.clone()) {
            backlight.process_event(&event);
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

                    let btn_defs = build_button_defs(&layers[active_layer]);
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
                                }
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

use chrono::{Local, Timelike};
use drm::control::ClipRect;
use input::event::{device::DeviceEvent, keyboard::*, Event, EventTrait};
use input::{Device as InputDevice, Libinput, LibinputInterface};
use input_linux::{uinput::UInputHandle, EventKind, Key, SynchronizeKind};
use input_linux_sys::{input_event, input_id, timeval, uinput_setup};
use libc::{c_char, O_ACCMODE, O_RDONLY, O_RDWR, O_WRONLY};
use nix::sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags};
use nix::sys::signal::{SigSet, Signal};
use privdrop::PrivDrop;
use std::cmp::min;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::{fs::OpenOptionsExt, io::OwnedFd};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::time::Instant;
use udev::MonitorBuilder;

mod backlight;
mod battery_icon_widget;
mod config;
mod display;
mod iced_renderer;
mod layer_manager;
mod memory_graph;
mod memory_graph_widget;
mod pixel_shift;
mod reconnect;
mod session_detect;
mod volume;
mod widgets;
mod workspace;

use backlight::BacklightManager;
use display::DrmBackend;
use iced_renderer::TouchbarRenderer;
use layer_manager::LayerManager;
use pixel_shift::PixelShiftManager;
use reconnect::ReconnectWatcher;
use widgets::{MainLoopAction, Message, RenderContext, WidgetAction};

const TIMEOUT_MS: i32 = 10 * 1000;
const RECONNECT_COOLDOWN_SECS: u64 = 2;

struct Interface;
impl LibinputInterface for Interface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        let mode = flags & O_ACCMODE;
        OpenOptions::new()
            .custom_flags(flags)
            .read(mode == O_RDONLY || mode == O_RDWR)
            .write(mode == O_WRONLY || mode == O_RDWR)
            .open(path)
            .map(|f| f.into())
            .map_err(|e| e.raw_os_error().unwrap())
    }
    fn close_restricted(&mut self, fd: OwnedFd) {
        _ = File::from(fd);
    }
}

fn emit<F: AsRawFd>(uinput: &mut UInputHandle<F>, ty: EventKind, code: u16, value: i32) {
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

fn toggle_keys<F: AsRawFd>(uinput: &mut UInputHandle<F>, codes: &[Key], value: i32) {
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

fn main() {
    let mut drm = DrmBackend::open_card().unwrap();
    let (height, width) = drm.mode().size();
    let _ = panic::catch_unwind(AssertUnwindSafe(|| real_main(&mut drm)));
    let (crash, mut wptr) = (include_bytes!("crash_bitmap.raw"), 0usize);
    let mut map = drm.map().unwrap();
    for byte in crash {
        for i in 0..8 {
            let c = if ((byte >> i) & 1) == 0 { 0xFF } else { 0x0 };
            map.as_mut()[wptr..wptr + 4].fill(c);
            wptr += 4;
        }
    }
    drop(map);
    drm.dirty(&[ClipRect::new(0, 0, height, width)]).unwrap();
    let mut ss = SigSet::empty();
    ss.add(Signal::SIGTERM);
    ss.wait().unwrap();
}

fn real_main(drm: &mut DrmBackend) {
    let (height, width) = drm.mode().size();
    let (db_width, _) = drm.fb_info().unwrap().size();
    let mut uinput = UInputHandle::new(OpenOptions::new().write(true).open("/dev/uinput").unwrap());
    let mut backlight = BacklightManager::new();
    let mut pixel_shift = PixelShiftManager::new();

    // Privilege drop
    let session_user = session_detect::detect_graphical_session_user();
    let drop_username = match &session_user {
        Some(u) => {
            eprintln!("Detected session user: {} (uid={})", u.username, u.uid);
            u.username.as_str()
        }
        None => {
            eprintln!("Warning: no graphical session, falling back to nobody");
            "nobody"
        }
    };
    PrivDrop::default()
        .user(drop_username)
        .group_list(&["input", "video"])
        .include_default_supplementary_groups()
        .apply()
        .unwrap_or_else(|e| panic!("Failed to drop privileges: {e}"));

    if let Some(ref user) = session_user {
        let xdg = format!("/run/user/{}", user.uid);
        std::env::set_var("XDG_RUNTIME_DIR", &xdg);
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={xdg}/bus"));
        if let Some(sock) = session_detect::discover_niri_socket(user.uid) {
            eprintln!("Discovered NIRI_SOCKET: {sock}");
            std::env::set_var("NIRI_SOCKET", &sock);
        } else {
            eprintln!("Warning: could not discover NIRI_SOCKET");
        }
    }

    // Input setup
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
    // data=2 is registered by LayerManager::new() for config fd
    epoll
        .add(&udev_monitor, EpollEvent::new(EpollFlags::EPOLLIN, 3))
        .unwrap();

    let mut layer_mgr = LayerManager::new(width, &epoll);
    let cfg = layer_mgr.config();
    let mut iced_rndr = TouchbarRenderer::new(
        width as u32,
        height as u32,
        db_width as u32,
        &cfg.font_family,
        cfg.font_size,
        cfg.font_bold,
        cfg.font_italic,
    );

    // Uinput setup
    uinput.set_evbit(EventKind::Key).unwrap();
    for k in &[Key::VolumeDown, Key::VolumeUp, Key::Esc] {
        uinput.set_keybit(*k).unwrap();
    }
    for k in &layer_mgr.all_key_actions() {
        uinput.set_keybit(*k).unwrap();
    }
    let mut dev_name_c = [0 as c_char; 80];
    let dn = "Dynamic Function Row Virtual Input Device".as_bytes();
    for i in 0..dn.len() {
        dev_name_c[i] = dn[i] as c_char;
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

    let xdg_runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }));
    let mut reconnect_watcher = ReconnectWatcher::new(&xdg_runtime_dir);
    epoll
        .add(
            reconnect_watcher.fd(),
            EpollEvent::new(EpollFlags::EPOLLIN, 6),
        )
        .unwrap();
    let mut last_reconnect: Option<Instant> = None;

    let mut needs_redraw = true;
    let mut blink_on = true;
    let mut last_blink = Instant::now();
    let mut battery_time_until: Option<Instant> = None;
    let mut digitizer: Option<InputDevice> = None;
    let mut touch_positions: HashMap<u32, iced_core::Point> = HashMap::new();

    loop {
        if layer_mgr.check_config_reload(&epoll) {
            let cfg = layer_mgr.config();
            iced_rndr = TouchbarRenderer::new(
                width as u32,
                height as u32,
                db_width as u32,
                &cfg.font_family,
                cfg.font_size,
                cfg.font_bold,
                cfg.font_italic,
            );
            needs_redraw = true;
        }

        let now = Local::now();
        let cfg = layer_mgr.config();
        let mut timeout = min(((60 - now.second()) * 1000) as i32, TIMEOUT_MS);
        if cfg.enable_pixel_shift {
            let (ps_redraw, ps_t) = pixel_shift.update();
            if ps_redraw {
                needs_redraw = true;
            }
            timeout = min(timeout, ps_t);
        }
        // Shorten timeout for widgets that need faster refresh (e.g. time with seconds).
        // The actual redraw decision comes from widget update() returning true.
        if layer_mgr.needs_faster_refresh() {
            timeout = min(timeout, 1000);
        }

        let now_i = Instant::now();
        if layer_mgr.needs_blink() {
            if now_i.duration_since(last_blink).as_millis() >= 500 {
                blink_on = !blink_on;
                last_blink = now_i;
                needs_redraw = true;
            }
            timeout = min(
                timeout,
                (500 - now_i.duration_since(last_blink).as_millis() as i32).max(1),
            );
        }

        if let Some(deadline) = battery_time_until {
            if now_i >= deadline {
                battery_time_until = None;
                needs_redraw = true;
            } else {
                timeout = min(timeout, deadline.duration_since(now_i).as_millis() as i32);
            }
        }

        if layer_mgr.update() {
            needs_redraw = true;
        }

        if needs_redraw {
            let ctx = RenderContext {
                font: iced_rndr.font(),
                font_size: iced_rndr.font_size(),
                blink_on,
                show_battery_time: battery_time_until.is_some(),
                window_title: layer_mgr.window_title(),
            };
            let buf = iced_rndr.render_widgets(layer_mgr.active_widgets(), &ctx);
            drm.map().unwrap().as_mut()[..buf.len()].copy_from_slice(&buf);
            drm.dirty(&[ClipRect::new(0, 0, height, width)]).unwrap();
            needs_redraw = false;
        }

        // Drain widget fds right before blocking to clear signals that
        // accumulated during rendering/processing. Background threads
        // (niri, PulseAudio) continuously signal eventfds; if we don't
        // drain here, epoll.wait() returns immediately every time.
        if layer_mgr.poll() {
            needs_redraw = true;
        }

        let _ = epoll.wait(
            &mut [EpollEvent::new(EpollFlags::EPOLLIN, 0)],
            timeout as u16,
        );
        _ = udev_monitor.iter().last();
        if layer_mgr.poll() {
            needs_redraw = true;
        }

        // Reconnection
        let re = reconnect_watcher.check_events();
        reconnect_watcher.ensure_watches();
        if re.niri || re.pulse {
            if layer_mgr.reconnect() {
                needs_redraw = true;
            }
        }
        if layer_mgr.any_disconnected()
            && last_reconnect.map_or(true, |t| t.elapsed().as_secs() >= RECONNECT_COOLDOWN_SECS)
        {
            last_reconnect = Some(Instant::now());
            if layer_mgr.reconnect() {
                needs_redraw = true;
            }
        }

        // Input
        if let Err(e) = input_tb.dispatch() {
            eprintln!("Warning: touchbar dispatch: {e}");
        }
        if let Err(e) = input_main.dispatch() {
            eprintln!("Warning: main dispatch: {e}");
        }
        for event in &mut input_tb.clone().chain(input_main.clone()) {
            backlight.process_event(&event);
            if backlight.take_lid_opened() {
                needs_redraw = true;
            }
            match event {
                Event::Device(DeviceEvent::Added(evt)) => {
                    if evt.device().name().contains(" Touch Bar") {
                        digitizer = Some(evt.device());
                    }
                }
                Event::Keyboard(KeyboardEvent::Key(key)) if key.key() == Key::Fn as u32 => {
                    let nl = if key.key_state() == KeyState::Pressed {
                        1
                    } else {
                        0
                    };
                    if layer_mgr.active_layer() != nl {
                        layer_mgr.switch_layer(nl);
                        needs_redraw = true;
                    }
                }
                Event::Touch(te) => {
                    if Some(te.device()) != digitizer || backlight.current_bl() == 0 {
                        continue;
                    }
                    let Some((iced_evt, cursor)) =
                        iced_renderer::translate_touch(&te, &mut touch_positions, width, height)
                    else {
                        continue;
                    };
                    let ctx = RenderContext {
                        font: iced_rndr.font(),
                        font_size: iced_rndr.font_size(),
                        blink_on,
                        show_battery_time: battery_time_until.is_some(),
                        window_title: layer_mgr.window_title(),
                    };
                    for msg in iced_rndr.process_touch_widgets(
                        iced_evt,
                        cursor,
                        layer_mgr.active_widgets(),
                        &ctx,
                    ) {
                        dispatch_message(
                            msg,
                            &mut layer_mgr,
                            &mut uinput,
                            &mut battery_time_until,
                            &mut needs_redraw,
                        );
                    }
                }
                _ => {}
            }
        }
        backlight.update_backlight(layer_mgr.config());
    }
}

fn dispatch_message<F: AsRawFd>(
    msg: Message,
    layer_mgr: &mut LayerManager,
    uinput: &mut UInputHandle<F>,
    btu: &mut Option<Instant>,
    redraw: &mut bool,
) {
    let widget_action = |layer_mgr: &mut LayerManager,
                         idx: usize,
                         action,
                         uinput: &mut UInputHandle<F>,
                         btu: &mut Option<Instant>,
                         redraw: &mut bool| {
        let layer = layer_mgr.active_widgets_mut();
        if idx < layer.len() {
            let actions: Vec<_> = layer[idx].handle_event(action);
            for a in actions {
                match &a {
                    MainLoopAction::SendKeys(k, p) => {
                        toggle_keys(uinput, k, if *p { 1 } else { 0 });
                        *redraw = true;
                    }
                    MainLoopAction::FocusWorkspace(id) => layer_mgr.focus_workspace(*id),
                    MainLoopAction::TriggerRedraw => *redraw = true,
                    MainLoopAction::ShowBatteryTime => {
                        *btu = Some(Instant::now() + std::time::Duration::from_secs(2));
                        *redraw = true;
                    }
                }
            }
        }
    };
    match msg {
        Message::WidgetPressed(i) => {
            widget_action(layer_mgr, i, WidgetAction::Pressed, uinput, btu, redraw)
        }
        Message::WidgetReleased(i) => {
            widget_action(layer_mgr, i, WidgetAction::Released, uinput, btu, redraw)
        }
        Message::WorkspaceDown(_) => *redraw = true,
        Message::WorkspaceUp(id) => layer_mgr.focus_workspace(id),
        Message::VolumeDownPress => toggle_keys(uinput, &[Key::VolumeDown], 1),
        Message::VolumeDownRelease => {
            toggle_keys(uinput, &[Key::VolumeDown], 0);
            *redraw = true;
        }
        Message::VolumeUpPress => toggle_keys(uinput, &[Key::VolumeUp], 1),
        Message::VolumeUpRelease => {
            toggle_keys(uinput, &[Key::VolumeUp], 0);
            *redraw = true;
        }
    }
}

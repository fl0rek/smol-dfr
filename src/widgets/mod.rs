pub mod battery;
pub mod load;
pub mod memory;
pub mod static_button;
pub mod temperature;
pub mod time;
pub mod volume;
pub mod window_title;
pub mod workspace;

use iced_core::font::Font;
use iced_core::{Background, Border, Color, Element, Theme};
use iced_widget::container;
use std::os::fd::BorrowedFd;
use std::path::Path;

pub type IcedRenderer = iced_tiny_skia::Renderer;

/// Shared context passed to render().
pub struct RenderContext {
    pub font: Font,
    pub font_size: f32,
    pub blink_on: bool,
    pub show_battery_time: bool,
    pub window_title: String,
}

/// Actions that widgets can request from the main loop.
#[derive(Debug, Clone)]
pub enum MainLoopAction {
    SendKeys(Vec<input_linux::Key>, bool),
    FocusWorkspace(u64),
    TriggerRedraw,
    ShowBatteryTime,
}

/// High-level touch actions delivered to widgets.
#[derive(Debug, Clone, Copy)]
pub enum WidgetAction {
    Pressed,
    Released,
}

/// iced Message type for widget interactions.
#[derive(Debug, Clone)]
pub enum Message {
    /// Generic widget press (index in current layer).
    WidgetPressed(usize),
    /// Generic widget release (index in current layer).
    WidgetReleased(usize),
    /// Workspace indicator pressed.
    WorkspaceDown(u64),
    /// Workspace indicator released (triggers focus).
    WorkspaceUp(u64),
    /// Volume down zone pressed.
    VolumeDownPress,
    /// Volume down zone released.
    VolumeDownRelease,
    /// Volume up zone pressed.
    VolumeUpPress,
    /// Volume up zone released.
    VolumeUpRelease,
}

/// Centralized epoll fd registration for widgets.
pub struct FdRegistry {
    entries: Vec<(u64, usize)>,
    next_data: u64,
}

impl FdRegistry {
    pub fn new(start_data: u64) -> Self {
        Self {
            entries: Vec::new(),
            next_data: start_data,
        }
    }

    /// Register a widget's fds with epoll. Stores the (data, widget_idx) mapping.
    pub fn register(
        &mut self,
        epoll: &nix::sys::epoll::Epoll,
        widget_idx: usize,
        fds: &[BorrowedFd],
    ) {
        use nix::sys::epoll::{EpollEvent, EpollFlags};
        for fd in fds {
            let ev = EpollEvent::new(EpollFlags::EPOLLIN, self.next_data);
            if let Err(e) = epoll.add(*fd, ev) {
                eprintln!("Warning: failed to register widget fd with epoll: {e}");
            } else {
                self.entries.push((self.next_data, widget_idx));
            }
            self.next_data += 1;
        }
    }

    /// Look up which widget index owns a given epoll data value.
    pub fn widget_for_data(&self, data: u64) -> Option<usize> {
        self.entries
            .iter()
            .find(|(d, _)| *d == data)
            .map(|(_, idx)| *idx)
    }
}

/// The Widget trait: object-safe, all display types implement this.
pub trait Widget {
    /// Build the iced Element for this widget's current state.
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer>;

    /// Called each loop iteration. Returns true if state changed (triggers redraw).
    fn update(&mut self) -> bool;

    /// Width fraction (0.0-1.0) this widget occupies.
    fn width_fraction(&self) -> f64;

    /// File descriptors to register with epoll.
    fn event_fds(&self) -> Vec<BorrowedFd<'_>> {
        vec![]
    }

    /// Called when one of this widget's fds fires in epoll. Returns true if state changed.
    fn poll(&mut self) -> bool {
        false
    }

    /// Handle high-level touch action. Returns actions for the main loop.
    fn handle_event(&mut self, action: WidgetAction) -> Vec<MainLoopAction>;

    /// Whether this widget needs faster refresh (e.g., seconds display).
    fn needs_faster_refresh(&self) -> bool {
        false
    }

    /// Whether the widget is currently connected/available.
    fn is_connected(&self) -> bool {
        true
    }

    /// Attempt reconnection. Returns true on success.
    fn try_connect(&mut self) -> bool {
        true
    }

    /// Return focused window title if this widget provides one (e.g. WorkspaceWidget).
    fn window_title(&self) -> Option<String> {
        None
    }

    /// Focus a workspace by id, if this widget manages workspaces.
    fn focus_workspace_if_applicable(&self, _id: u64) {}
}

/// Build a widget layer from button configs.
pub fn build_widget_layer(
    cfgs: &[crate::config::ButtonConfig],
    ws_cfg: Option<&crate::config::WorkspacesConfig>,
    vol_cfg: Option<&crate::config::VolumeConfig>,
) -> Vec<Box<dyn Widget>> {
    if cfgs.is_empty() {
        panic!("Invalid configuration, layer has 0 buttons");
    }

    let specified_total: f64 = cfgs.iter().filter_map(|c| c.width).sum();
    let unspecified_count = cfgs.iter().filter(|c| c.width.is_none()).count();
    let remaining = (100.0 - specified_total).max(0.0);
    let default_width = if unspecified_count > 0 {
        remaining / unspecified_count as f64
    } else {
        0.0
    };

    cfgs.iter()
        .map(|c| {
            let frac = c.width.unwrap_or(default_width) / 100.0;
            let color = c.color;
            let action = c.action.clone();

            let w: Box<dyn Widget> = if let Some(ref time_fmt) = c.time {
                Box::new(time::TimeWidget::new(time_fmt, c.locale.as_deref(), frac, action, color))
            } else if let Some(ref battery_mode) = c.battery {
                match battery::BatteryWidget::try_new(battery_mode, action.clone(), frac, color) {
                    Some(bw) => Box::new(bw),
                    None => Box::new(static_button::StaticButton::new_text(
                        "Battery N/A".to_string(), action, frac, color,
                    )),
                }
            } else if c.temperature == Some(true) {
                match temperature::TemperatureWidget::try_new(frac, color) {
                    Some(tw) => Box::new(tw),
                    None => Box::new(static_button::StaticButton::new_text(
                        "Temp N/A".to_string(), vec![], frac, color,
                    )),
                }
            } else if c.load_avg == Some(true) {
                Box::new(load::LoadAvgWidget::new(frac, color))
            } else if c.memory == Some(true) {
                let sample_interval = c.sample_interval.unwrap_or(1000);
                let graph_window = c.graph_window.unwrap_or(60);
                Box::new(memory::MemoryWidget::new(sample_interval, graph_window, frac, color))
            } else if c.workspaces == Some(true) {
                if let Some(ws) = ws_cfg {
                    let mut widget = workspace::WorkspaceWidget::new(ws.provider.as_deref(), ws, frac);
                    if !widget.try_connect() {
                        eprintln!("Warning: [Workspaces] configured but could not connect (will reconnect when available)");
                    }
                    Box::new(widget)
                } else {
                    Box::new(static_button::StaticButton::new_spacer(frac, color))
                }
            } else if c.window_title == Some(true) {
                Box::new(window_title::WindowTitleWidget::new(frac, color))
            } else if c.volume == Some(true) {
                if let Some(vol) = vol_cfg {
                    let mut widget = volume::VolumeWidget::new(vol.pulse_server.as_deref(), frac, color);
                    if !widget.try_connect() {
                        eprintln!("Warning: [Volume] configured but could not connect (will reconnect when available)");
                    }
                    Box::new(widget)
                } else {
                    Box::new(static_button::StaticButton::new_spacer(frac, color))
                }
            } else if let Some(ref label) = c.text {
                Box::new(static_button::StaticButton::new_text(label.clone(), action, frac, color))
            } else if let Some(ref icon_name) = c.icon {
                Box::new(static_button::StaticButton::new_icon(icon_name, action, frac, color))
            } else {
                Box::new(static_button::StaticButton::new_spacer(frac, color))
            };
            w
        })
        .collect()
}

/// Get the window title from the workspace widget, if any.
pub fn get_window_title(widgets: &[Box<dyn Widget>]) -> String {
    for widget in widgets {
        if let Some(title) = widget.window_title() {
            return title;
        }
    }
    String::new()
}

/// Focus a workspace by finding the WorkspaceWidget and calling focus_workspace.
pub fn focus_workspace(widgets: &[Box<dyn Widget>], id: u64) {
    for widget in widgets {
        widget.focus_workspace_if_applicable(id);
    }
}

/// Resolve an icon name to an SVG file path.
pub(crate) fn resolve_icon_path(name: &str) -> Option<String> {
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

/// Compute the rounded-rect background style used by all widgets.
pub fn button_style(color: Option<(f64, f64, f64)>, active: bool) -> container::Style {
    let bg = match color {
        Some((r, g, b)) => {
            let scale = if active { 1.0 } else { 0.5 };
            Color::from_rgb(r as f32 * scale, g as f32 * scale, b as f32 * scale)
        }
        None => {
            let v = if active { 0.4 } else { 0.2 };
            Color::from_rgb(v, v, v)
        }
    };
    container::Style {
        background: Some(Background::Color(bg)),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub mod battery;
pub mod static_button;
pub mod temperature;
pub mod time;

use iced_core::alignment;
use iced_core::font::Font;
use iced_core::{Background, Border, Color, Element, Length, Theme};
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

/// iced Message type used by the new widget rendering path.
/// Keeps backward-compatible variants from the old Message enum during migration.
#[derive(Debug, Clone)]
pub enum Message {
    // New generic widget messages
    WidgetPressed(usize),
    WidgetReleased(usize),
    // Legacy variants kept during migration
    ButtonDown(usize),
    ButtonUp(usize),
    WorkspaceDown(u64),
    WorkspaceUp(u64),
    VolumeDownPress,
    VolumeDownRelease,
    VolumeUpPress,
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

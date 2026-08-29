pub mod battery;
pub mod load;
pub mod memory;
pub mod static_button;
pub mod temperature;
pub mod time;
pub mod volume;
pub mod window_title;
pub mod workspace;

use iced_core::alignment;
use iced_core::font::Font;
use iced_core::{Background, Border, Color, Element, Length, Theme};
use iced_widget::{container, text};
use std::os::fd::BorrowedFd;
use std::path::Path;

pub type IcedRenderer = iced_tiny_skia::Renderer;

/// Shared context passed to `render()`.
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
/// Tracks registered fds so they can be removed before widgets are dropped.
pub(crate) struct FdRegistry {
    next_data: u64,
}

impl FdRegistry {
    pub const fn new(start_data: u64) -> Self {
        Self {
            next_data: start_data,
        }
    }

    /// Register all widget fds from both layers with epoll.
    pub fn register_all(
        &mut self,
        epoll: &nix::sys::epoll::Epoll,
        layers: &[Vec<Box<dyn Widget>>; 2],
    ) {
        use nix::sys::epoll::{EpollEvent, EpollFlags};
        for layer in layers {
            for widget in layer {
                for fd in widget.event_fds() {
                    let ev = EpollEvent::new(EpollFlags::EPOLLIN, self.next_data);
                    if let Err(e) = epoll.add(fd, ev) {
                        eprintln!("Warning: failed to register widget fd with epoll: {e}");
                    }
                    self.next_data += 1;
                }
            }
        }
    }

    /// Remove all widget fds from epoll. Must be called while widgets are
    /// still alive so the fd numbers remain valid.
    pub fn unregister_all(epoll: &nix::sys::epoll::Epoll, layers: &[Vec<Box<dyn Widget>>; 2]) {
        for layer in layers {
            for widget in layer {
                for fd in widget.event_fds() {
                    let _ = epoll.delete(fd);
                }
            }
        }
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
    fn handle_event(&mut self, _action: WidgetAction) -> Vec<MainLoopAction> {
        vec![]
    }

    /// Whether this widget currently needs blink redraws (e.g., low battery blinking).
    fn needs_blink(&self) -> bool {
        false
    }

    /// How often this widget needs `update()` called, in milliseconds.
    ///
    /// `None` means the widget has no timing requirement of its own and is
    /// content with the main loop's default timeout. The main loop shortens
    /// its epoll timeout to the smallest interval any active widget asks for,
    /// so a widget that samples on a schedule must report it here or it will
    /// simply not be polled often enough.
    fn refresh_interval_ms(&self) -> Option<u32> {
        None
    }

    /// Whether the widget is currently connected/available.
    fn is_connected(&self) -> bool {
        true
    }

    /// Attempt reconnection. Returns true on success.
    fn try_connect(&mut self) -> bool {
        true
    }

    /// Return focused window title if this widget provides one (e.g. `WorkspaceWidget`).
    fn window_title(&self) -> Option<String> {
        None
    }

    /// Focus a workspace by id, if this widget manages workspaces.
    fn focus_workspace_if_applicable(&self, _id: u64) {}
}

/// Build a widget layer from widget entries.
pub(crate) fn build_widget_layer(
    entries: &[crate::config::WidgetEntry],
    ws_cfg: Option<&crate::config::WorkspacesConfig>,
    vol_cfg: Option<&crate::config::VolumeConfig>,
) -> Vec<Box<dyn Widget>> {
    use crate::config::WidgetConfig;

    assert!(
        !entries.is_empty(),
        "Invalid configuration, layer has 0 buttons"
    );

    let specified_total: f64 = entries.iter().filter_map(|e| e.width).sum();
    let unspecified_count = entries.iter().filter(|e| e.width.is_none()).count();
    let remaining = (100.0_f64 - specified_total).max(0.0);
    let default_width = if unspecified_count > 0 {
        remaining / unspecified_count as f64
    } else {
        0.0
    };

    entries.iter()
        .map(|entry| {
            let frac = entry.width.unwrap_or(default_width) / 100.0;
            let color = entry.color;
            let action = entry.action.clone();

            let w: Box<dyn Widget> = match &entry.widget {
                WidgetConfig::Text { text } => {
                    Box::new(static_button::StaticButton::new_text(text.clone(), action, frac, color))
                }
                WidgetConfig::Icon { icon } => {
                    Box::new(static_button::StaticButton::new_icon(icon, action, frac, color))
                }
                WidgetConfig::Time { format, locale } => {
                    Box::new(time::TimeWidget::new(format, locale.as_deref(), frac, action, color))
                }
                WidgetConfig::Battery => {
                    match battery::BatteryWidget::try_new(action.clone(), frac, color) {
                        Some(bw) => Box::new(bw),
                        None => Box::new(static_button::StaticButton::new_text(
                            "Battery N/A".into(), action, frac, color,
                        )),
                    }
                }
                WidgetConfig::Temperature => {
                    match temperature::TemperatureWidget::try_new(frac, color) {
                        Some(tw) => Box::new(tw),
                        None => Box::new(static_button::StaticButton::new_text(
                            "Temp N/A".into(), vec![], frac, color,
                        )),
                    }
                }
                WidgetConfig::LoadAvg => Box::new(load::LoadAvgWidget::new(frac, color)),
                WidgetConfig::Memory { sample_interval, graph_window } => {
                    Box::new(memory::MemoryWidget::new(
                        sample_interval.unwrap_or(1000),
                        graph_window.unwrap_or(60),
                        frac, color,
                    ))
                }
                WidgetConfig::Workspaces => {
                    if let Some(ws) = ws_cfg {
                        let mut widget = workspace::WorkspaceWidget::new(ws, frac);
                        if !widget.try_connect() {
                            eprintln!("Warning: [Workspaces] configured but could not connect (will reconnect when available)");
                        }
                        Box::new(widget)
                    } else {
                        Box::new(static_button::StaticButton::new_spacer(frac, color))
                    }
                }
                WidgetConfig::WindowTitle => {
                    Box::new(window_title::WindowTitleWidget::new(frac, color))
                }
                WidgetConfig::Volume => {
                    if let Some(vol) = vol_cfg {
                        let mut widget = volume::VolumeWidget::new(vol.pulse_server.as_deref(), frac, color);
                        if !widget.try_connect() {
                            eprintln!("Warning: [Volume] configured but could not connect (will reconnect when available)");
                        }
                        Box::new(widget)
                    } else {
                        Box::new(static_button::StaticButton::new_spacer(frac, color))
                    }
                }
                WidgetConfig::Spacer => {
                    Box::new(static_button::StaticButton::new_spacer(frac, color))
                }
            };
            w
        })
        .collect()
}

/// Resolve an icon name to an SVG file path.
pub(crate) fn resolve_icon_path(name: &str) -> Option<String> {
    let candidates = [
        format!("/etc/smol-dfr/{name}.svg"),
        format!("/usr/share/smol-dfr/{name}.svg"),
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
    let bg = if let Some((r, g, b)) = color {
        let scale = if active { 1.0 } else { 0.5 };
        Color::from_rgb(r as f32 * scale, g as f32 * scale, b as f32 * scale)
    } else {
        let v = if active { 0.4 } else { 0.2 };
        Color::from_rgb(v, v, v)
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

/// Build a styled text container with the standard widget appearance.
/// Uses the default font size and white text color.
pub fn styled_text_widget<'a>(
    label: impl Into<String>,
    ctx: &RenderContext,
    color: Option<(f64, f64, f64)>,
    active: bool,
) -> Element<'a, Message, Theme, IcedRenderer> {
    styled_text_widget_with(label, ctx, color, active, ctx.font_size, Color::WHITE)
}

/// Handle press/release key-action pattern common to widgets with key bindings.
/// Toggles `active` state and emits `SendKeys` actions.
pub fn handle_key_action(
    active: &mut bool,
    keys: &[input_linux::Key],
    action: WidgetAction,
) -> Vec<MainLoopAction> {
    if keys.is_empty() {
        return vec![];
    }
    match action {
        WidgetAction::Pressed => {
            *active = true;
            vec![MainLoopAction::SendKeys(keys.to_vec(), true)]
        }
        WidgetAction::Released => {
            *active = false;
            vec![MainLoopAction::SendKeys(keys.to_vec(), false)]
        }
    }
}

/// Build a styled text container with custom font size and text color.
pub fn styled_text_widget_with<'a>(
    label: impl Into<String>,
    ctx: &RenderContext,
    color: Option<(f64, f64, f64)>,
    active: bool,
    font_size: f32,
    text_color: Color,
) -> Element<'a, Message, Theme, IcedRenderer> {
    container(
        text(label.into())
            .font(ctx.font)
            .size(font_size)
            .color(text_color)
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(2)
    .style(move |_theme: &Theme| button_style(color, active))
    .into()
}

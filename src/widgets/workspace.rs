use iced_core::alignment;
use iced_core::{Color, Element, Length, Theme};
use iced_widget::{container, mouse_area, row, text};

use crate::config::WorkspacesConfig;
use crate::workspace::WorkspaceManager;

use super::{
    button_style, IcedRenderer, MainLoopAction, Message, RenderContext, Widget, WidgetAction,
};
use std::os::fd::BorrowedFd;

pub struct WorkspaceWidget {
    manager: WorkspaceManager,
    active_color: (f64, f64, f64),
    urgent_color: (f64, f64, f64),
    width_fraction: f64,
}

impl WorkspaceWidget {
    pub fn new(provider: Option<&str>, ws_cfg: &WorkspacesConfig, width_fraction: f64) -> Self {
        Self {
            manager: WorkspaceManager::new(provider),
            active_color: ws_cfg.active_color,
            urgent_color: ws_cfg.urgent_color,
            width_fraction,
        }
    }

    /// Get the focused window title from the workspace manager.
    pub fn focused_window_title(&self) -> Option<String> {
        self.manager.focused_window_title()
    }

    /// Focus a workspace by id.
    pub fn focus_workspace(&self, id: u64) {
        self.manager.focus_workspace(id);
    }

    /// Check and clear the reconnect flash flag.
    pub fn has_reconnect_flash(&self) -> bool {
        self.manager.has_reconnect_flash()
    }
}

impl Widget for WorkspaceWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let workspaces = self.manager.workspaces();

        if workspaces.is_empty() || !self.manager.is_connected() {
            // Return minimal empty container
            return container(text(""))
                .width(Length::Shrink)
                .height(Length::Fill)
                .into();
        }

        let children: Vec<Element<'_, Message, Theme, IcedRenderer>> = workspaces
            .iter()
            .map(|w| {
                let color = if w.is_urgent {
                    Some(self.urgent_color)
                } else if w.is_focused {
                    Some(self.active_color)
                } else {
                    None
                };
                let style_active = w.is_focused;
                let label = w.name.clone().unwrap_or_else(|| w.idx.to_string());
                let id = w.id;

                let inner: Element<'_, Message, Theme, IcedRenderer> = container(
                    text(label)
                        .font(ctx.font)
                        .size(ctx.font_size)
                        .color(Color::WHITE)
                        .align_x(alignment::Horizontal::Center)
                        .align_y(alignment::Vertical::Center)
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(2)
                .style(move |_theme: &Theme| button_style(color, style_active))
                .into();

                let wrapped: Element<'_, Message, Theme, IcedRenderer> = mouse_area(inner)
                    .on_press(Message::WorkspaceDown(id))
                    .on_release(Message::WorkspaceUp(id))
                    .on_exit(Message::WorkspaceUp(id))
                    .into();

                container(wrapped)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            })
            .collect();

        container(
            row(children)
                .spacing(2)
                .height(Length::Fill)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn update(&mut self) -> bool {
        false
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }

    fn event_fds(&self) -> Vec<BorrowedFd<'_>> {
        vec![self.manager.event_fd()]
    }

    fn poll(&mut self) -> bool {
        self.manager.poll()
    }

    fn is_connected(&self) -> bool {
        self.manager.is_connected()
    }

    fn try_connect(&mut self) -> bool {
        self.manager.try_connect()
    }

    fn handle_event(&mut self, _action: WidgetAction) -> Vec<MainLoopAction> {
        vec![]
    }

    fn window_title(&self) -> Option<String> {
        self.manager.focused_window_title()
    }

    fn focus_workspace_if_applicable(&self, id: u64) {
        self.manager.focus_workspace(id);
    }
}

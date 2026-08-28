use iced_core::alignment;
use iced_core::{Color, ContentFit, Element, Length, Theme};
use iced_widget::{container, mouse_area, row, svg, text};

use crate::volume::VolumeManager;

use super::{button_style, resolve_icon_path, IcedRenderer, Message, RenderContext, Widget};
use std::os::fd::BorrowedFd;

pub struct VolumeWidget {
    manager: VolumeManager,
    down_handle: Option<svg::Handle>,
    up_handle: Option<svg::Handle>,
    width_fraction: f64,
    color: Option<(f64, f64, f64)>,
    active: bool,
}

impl VolumeWidget {
    pub fn new(
        pulse_server: Option<&str>,
        width_fraction: f64,
        color: Option<(f64, f64, f64)>,
    ) -> Self {
        Self {
            manager: VolumeManager::new(pulse_server),
            down_handle: resolve_icon_path("volume_down").map(svg::Handle::from_path),
            up_handle: resolve_icon_path("volume_up").map(svg::Handle::from_path),
            width_fraction,
            color,
            active: false,
        }
    }
}

impl Widget for VolumeWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let style_color = self.color;
        let style_active = self.active;

        let label = if !self.manager.is_connected() {
            "Vol N/A".to_string()
        } else {
            let vol = self.manager.volume();
            if vol.muted {
                "muted".to_string()
            } else {
                format!("{}%", vol.volume_percent)
            }
        };

        let make_icon =
            |handle: &Option<svg::Handle>| -> Element<'_, Message, Theme, IcedRenderer> {
                if let Some(h) = handle {
                    svg::Svg::new(h.clone())
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .content_fit(ContentFit::Contain)
                        .into()
                } else {
                    text("").width(Length::Fill).height(Length::Fill).into()
                }
            };

        let left: Element<'_, Message, Theme, IcedRenderer> = container(
            mouse_area(make_icon(&self.down_handle))
                .on_press(Message::VolumeDownPress)
                .on_release(Message::VolumeDownRelease)
                .on_exit(Message::VolumeDownRelease),
        )
        .width(Length::FillPortion(1))
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into();

        let center: Element<'_, Message, Theme, IcedRenderer> = container(
            text(label)
                .font(ctx.font)
                .size(ctx.font_size)
                .color(Color::WHITE)
                .align_x(alignment::Horizontal::Center)
                .align_y(alignment::Vertical::Center)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(Length::FillPortion(2))
        .height(Length::Fill)
        .into();

        let right: Element<'_, Message, Theme, IcedRenderer> = container(
            mouse_area(make_icon(&self.up_handle))
                .on_press(Message::VolumeUpPress)
                .on_release(Message::VolumeUpRelease)
                .on_exit(Message::VolumeUpRelease),
        )
        .width(Length::FillPortion(1))
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into();

        let vol_row = row(vec![left, center, right])
            .width(Length::Fill)
            .height(Length::Fill);

        container(
            container(vol_row)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(2)
                .style(move |_theme: &Theme| button_style(style_color, style_active)),
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
}

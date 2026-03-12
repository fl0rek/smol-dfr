use iced_core::alignment;
use iced_core::{Color, Element, Length, Theme};
use iced_widget::{container, text};

use super::{
    button_style, IcedRenderer, MainLoopAction, Message, RenderContext, Widget, WidgetAction,
};

pub struct WindowTitleWidget {
    width_fraction: f64,
    color: Option<(f64, f64, f64)>,
}

impl WindowTitleWidget {
    pub fn new(width_fraction: f64, color: Option<(f64, f64, f64)>) -> Self {
        Self {
            width_fraction,
            color,
        }
    }
}

impl Widget for WindowTitleWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let style_color = self.color;

        container(
            text(ctx.window_title.clone())
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
        .style(move |_theme: &Theme| button_style(style_color, false))
        .into()
    }

    fn update(&mut self) -> bool {
        false
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }

    fn handle_event(&mut self, _action: WidgetAction) -> Vec<MainLoopAction> {
        vec![]
    }
}

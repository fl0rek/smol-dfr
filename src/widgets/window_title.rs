use iced_core::{Element, Theme};

use super::{styled_text_widget, IcedRenderer, Message, RenderContext, Widget};

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

        styled_text_widget(ctx.window_title.clone(), ctx, style_color, false)
    }

    fn update(&mut self) -> bool {
        false
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }
}

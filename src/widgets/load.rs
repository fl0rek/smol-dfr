use iced_core::alignment;
use iced_core::{Color, Element, Length, Theme};
use iced_widget::{container, text};
use std::fs;

use super::{
    button_style, IcedRenderer, MainLoopAction, Message, RenderContext, Widget, WidgetAction,
};

pub(crate) fn get_load_avg() -> String {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(|v| v.to_string()))
        .unwrap_or_else(|| "--".to_string())
}

pub struct LoadAvgWidget {
    width_fraction: f64,
    color: Option<(f64, f64, f64)>,
    load_avg_failed: bool,
}

impl LoadAvgWidget {
    pub fn new(width_fraction: f64, color: Option<(f64, f64, f64)>) -> Self {
        Self {
            width_fraction,
            color,
            load_avg_failed: false,
        }
    }
}

impl Widget for LoadAvgWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let style_color = self.color;
        let reading = get_load_avg();

        container(
            text(reading)
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
        // Report load avg failure via log-once pattern
        let reading = get_load_avg();
        let ok = reading != "--";
        if ok && self.load_avg_failed {
            eprintln!("Load average recovered");
            self.load_avg_failed = false;
        } else if !ok && !self.load_avg_failed {
            eprintln!("Warning: /proc/loadavg read failed, showing '--'");
            self.load_avg_failed = true;
        }
        // Always return true: load may change each iteration
        true
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }

    fn handle_event(&mut self, _action: WidgetAction) -> Vec<MainLoopAction> {
        vec![]
    }
}

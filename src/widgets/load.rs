use iced_core::alignment;
use iced_core::{Color, Element, Length, Theme};
use iced_widget::{container, text};
use std::fs;
use std::time::Instant;

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
    last_reading: String,
    cached_reading: String,
    last_sysfs_read: Option<Instant>,
}

impl LoadAvgWidget {
    pub fn new(width_fraction: f64, color: Option<(f64, f64, f64)>) -> Self {
        Self {
            width_fraction,
            color,
            load_avg_failed: false,
            last_reading: String::new(),
            cached_reading: String::new(),
            last_sysfs_read: None,
        }
    }

    /// Refresh cached load average reading from procfs if at least 1 second has elapsed.
    fn refresh_if_needed(&mut self) {
        let stale = match self.last_sysfs_read {
            None => true,
            Some(t) => t.elapsed() >= std::time::Duration::from_secs(1),
        };
        if stale {
            self.cached_reading = get_load_avg();
            self.last_sysfs_read = Some(Instant::now());
        }
    }
}

impl Widget for LoadAvgWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let style_color = self.color;
        let reading = self.cached_reading.clone();

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
        self.refresh_if_needed();
        let reading = self.cached_reading.clone();
        // Report load avg failure via log-once pattern
        let ok = reading != "--";
        if ok && self.load_avg_failed {
            eprintln!("Load average recovered");
            self.load_avg_failed = false;
        } else if !ok && !self.load_avg_failed {
            eprintln!("Warning: /proc/loadavg read failed, showing '--'");
            self.load_avg_failed = true;
        }
        // Only trigger redraw when displayed load string changes
        let changed = reading != self.last_reading;
        self.last_reading = reading;
        changed
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }

    fn handle_event(&mut self, _action: WidgetAction) -> Vec<MainLoopAction> {
        vec![]
    }
}

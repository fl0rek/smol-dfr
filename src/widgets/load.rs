use iced_core::{Element, Theme};
use std::fs;
use std::time::Duration;

use crate::rate_limit::{LogOnce, RateLimitedValue};

use super::{styled_text_widget, IcedRenderer, Message, RenderContext, Widget};

pub(crate) fn get_load_avg() -> String {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(|v| v.to_string()))
        .unwrap_or_else(|| "--".to_string())
}

pub struct LoadAvgWidget {
    width_fraction: f64,
    color: Option<(f64, f64, f64)>,
    log_once: LogOnce,
    last_reading: String,
    cached_reading: RateLimitedValue<String>,
}

impl LoadAvgWidget {
    pub const fn new(width_fraction: f64, color: Option<(f64, f64, f64)>) -> Self {
        Self {
            width_fraction,
            color,
            log_once: LogOnce::new(
                "Warning: /proc/loadavg read failed, showing '--'",
                "Load average recovered",
            ),
            last_reading: String::new(),
            cached_reading: RateLimitedValue::new(String::new(), Duration::from_secs(1)),
        }
    }
}

impl Widget for LoadAvgWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let style_color = self.color;
        let reading = self.cached_reading.get().clone();

        styled_text_widget(reading, ctx, style_color, false)
    }

    fn update(&mut self) -> bool {
        self.cached_reading.refresh_if_needed(get_load_avg);
        let reading = self.cached_reading.get().clone();
        self.log_once.check(reading != "--");
        // Only trigger redraw when displayed load string changes
        let changed = reading != self.last_reading;
        self.last_reading = reading;
        changed
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }
}

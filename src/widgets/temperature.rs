use iced_core::{Element, Theme};
use std::fs;
use std::time::Duration;

use crate::rate_limit::{LogOnce, RateLimitedValue};

use super::{styled_text_widget, IcedRenderer, Message, RenderContext, Widget};

pub(crate) fn find_thermal_zone() -> Option<String> {
    let base = "/sys/class/thermal";
    // Prefer CPU-related zones, fall back to first available
    let mut best: Option<(String, i32)> = None;
    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("thermal_zone") {
                continue;
            }
            let type_str = fs::read_to_string(entry.path().join("type"))
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            let priority = if type_str.contains("cpu") || type_str.contains("soc") {
                2
            } else if type_str.contains("battery") || type_str.contains("gpu") {
                0
            } else {
                1
            };
            if best.as_ref().map_or(true, |(_, p)| priority > *p) {
                best = Some((name, priority));
            }
        }
    }
    best.map(|(name, _)| name)
}

pub(crate) fn get_temperature(zone: &str) -> String {
    let path = format!("/sys/class/thermal/{}/temp", zone);
    match fs::read_to_string(&path) {
        Ok(s) => match s.trim().parse::<f64>() {
            Ok(millideg) => format!("{:.0}\u{00B0}C", millideg / 1000.0),
            Err(_) => "--".to_string(),
        },
        Err(_) => "--".to_string(),
    }
}

pub struct TemperatureWidget {
    zone: String,
    width_fraction: f64,
    color: Option<(f64, f64, f64)>,
    log_once: LogOnce,
    last_reading: String,
    cached_reading: RateLimitedValue<String>,
}

impl TemperatureWidget {
    /// Create a new TemperatureWidget. Returns None if no thermal zone is found.
    pub fn try_new(width_fraction: f64, color: Option<(f64, f64, f64)>) -> Option<Self> {
        let zone = find_thermal_zone()?;
        Some(Self {
            zone,
            width_fraction,
            color,
            log_once: LogOnce::new(
                "Warning: thermal sysfs read failed, showing '--'",
                "Thermal sysfs recovered",
            ),
            last_reading: String::new(),
            cached_reading: RateLimitedValue::new(String::new(), Duration::from_secs(1)),
        })
    }
}

impl Widget for TemperatureWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let style_color = self.color;

        let reading = self.cached_reading.get().clone();

        styled_text_widget(reading, ctx, style_color, false)
    }

    fn update(&mut self) -> bool {
        let zone = self.zone.clone();
        self.cached_reading
            .refresh_if_needed(|| get_temperature(&zone));
        let reading = self.cached_reading.get().clone();
        self.log_once.check(reading != "--");
        // Only trigger redraw when displayed temperature string changes
        let changed = reading != self.last_reading;
        self.last_reading = reading;
        changed
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }
}

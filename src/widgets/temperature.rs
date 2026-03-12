use iced_core::alignment;
use iced_core::{Color, Element, Length, Theme};
use iced_widget::{container, text};
use std::fs;

use super::{
    button_style, IcedRenderer, MainLoopAction, Message, RenderContext, Widget, WidgetAction,
};

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
    thermal_failed: bool,
    last_reading: String,
}

impl TemperatureWidget {
    /// Create a new TemperatureWidget. Returns None if no thermal zone is found.
    pub fn try_new(width_fraction: f64, color: Option<(f64, f64, f64)>) -> Option<Self> {
        let zone = find_thermal_zone()?;
        Some(Self {
            zone,
            width_fraction,
            color,
            thermal_failed: false,
            last_reading: String::new(),
        })
    }
}

impl Widget for TemperatureWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let style_color = self.color;

        let reading = get_temperature(&self.zone);

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
        let reading = get_temperature(&self.zone);
        // Report thermal failure via log-once pattern
        let ok = reading != "--";
        if ok && self.thermal_failed {
            eprintln!("Thermal sysfs recovered");
            self.thermal_failed = false;
        } else if !ok && !self.thermal_failed {
            eprintln!("Warning: thermal sysfs read failed, showing '--'");
            self.thermal_failed = true;
        }
        // Only trigger redraw when displayed temperature string changes
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

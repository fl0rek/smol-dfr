use iced_core::alignment;
use iced_core::{Color, Element, Length, Theme};
use iced_widget::{container, text, Stack};
use std::fs;

use crate::battery_icon_widget::BatteryIconWidget;

use super::{
    button_style, IcedRenderer, MainLoopAction, Message, RenderContext, Widget, WidgetAction,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatteryState {
    NotCharging,
    Charging,
    Low,
}

pub(crate) fn find_battery_device() -> Option<String> {
    let power_supply_path = "/sys/class/power_supply";
    if let Ok(entries) = fs::read_dir(power_supply_path) {
        for entry in entries.flatten() {
            let dev_path = entry.path();
            let type_path = dev_path.join("type");
            if let Ok(typ) = fs::read_to_string(&type_path) {
                if typ.trim() == "Battery" {
                    if let Some(name) = dev_path.file_name().and_then(|n| n.to_str()) {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    None
}

pub(crate) fn get_battery_state(battery: &str) -> Option<(u32, BatteryState)> {
    let status_path = format!("/sys/class/power_supply/{}/status", battery);
    let status = fs::read_to_string(&status_path)
        .unwrap_or_else(|_| "Unknown".to_string());

    let capacity = {
        let base = format!("/sys/class/power_supply/{}", battery);
        let from_ratio = |num_file: &str, den_file: &str| -> Option<u32> {
            let num = fs::read_to_string(format!("{}/{}", base, num_file))
                .ok()?.trim().parse::<f64>().ok()?;
            let den = fs::read_to_string(format!("{}/{}", base, den_file))
                .ok()?.trim().parse::<f64>().ok().filter(|v| *v > 0.0)?;
            Some(((num / den) * 100.0).round() as u32)
        };
        from_ratio("charge_now", "charge_full")
            .or_else(|| from_ratio("energy_now", "energy_full"))
            .or_else(|| {
                fs::read_to_string(format!("{}/capacity", base))
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
            })?
    };

    let state = match status.trim() {
        "Charging" | "Full" => BatteryState::Charging,
        "Discharging" if capacity < 10 => BatteryState::Low,
        _ => BatteryState::NotCharging,
    };
    Some((capacity, state))
}

/// Returns estimated time remaining as a formatted string (e.g. "7h51m" or "1h23m").
/// Returns None if estimation is not possible.
pub(crate) fn get_battery_time_estimate(battery: &str, charging: bool) -> Option<String> {
    let base = format!("/sys/class/power_supply/{}", battery);
    let read_val = |file: &str| -> Option<f64> {
        fs::read_to_string(format!("{}/{}", base, file))
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| *v != 0.0)
    };

    // 1. Try direct time_to_* files (value in seconds)
    let time_secs = if charging {
        read_val("time_to_full_now")
    } else {
        read_val("time_to_empty_now")
    };
    if let Some(secs) = time_secs {
        let h = (secs / 3600.0) as u32;
        let m = ((secs % 3600.0) / 60.0) as u32;
        return Some(format!("{}h{:02}m", h, m));
    }

    // 2. Try energy_now / power_now
    let energy_now = read_val("energy_now");
    let energy_full = read_val("energy_full");
    let power_now = read_val("power_now").map(|v| v.abs());

    // 3. Derive energy from charge x voltage if needed
    let (energy_now, energy_full) = match (energy_now, energy_full) {
        (Some(en), Some(ef)) => (Some(en), Some(ef)),
        _ => {
            let voltage = read_val("voltage_now")?;
            let cn = read_val("charge_now")?;
            let cf = read_val("charge_full")?;
            (
                Some(cn * voltage / 1_000_000.0),
                Some(cf * voltage / 1_000_000.0),
            )
        }
    };

    // 4. Derive power from current x voltage if needed
    let power = power_now.or_else(|| {
        let voltage = read_val("voltage_now")?;
        let current = read_val("current_now").map(|v| v.abs())?;
        Some(voltage * current / 1_000_000.0)
    });

    let (en, ef, pw) = (energy_now?, energy_full?, power.filter(|v| *v > 0.0)?);
    let hours = if charging { (ef - en) / pw } else { en / pw };
    let h = hours as u32;
    let m = ((hours - h as f64) * 60.0) as u32;
    Some(format!("{}h{:02}m", h, m))
}

pub struct BatteryWidget {
    battery_device: String,
    width_fraction: f64,
    action: Vec<input_linux::Key>,
    active: bool,
    color: Option<(f64, f64, f64)>,
    battery_failed: bool,
}

impl BatteryWidget {
    /// Create a new BatteryWidget. Returns None if no battery device is found.
    pub fn try_new(
        _battery_mode: &str,
        action: Vec<input_linux::Key>,
        width_fraction: f64,
        color: Option<(f64, f64, f64)>,
    ) -> Option<Self> {
        let battery_device = find_battery_device()?;
        Some(Self {
            battery_device,
            width_fraction,
            action,
            active: false,
            color,
            battery_failed: false,
        })
    }
}

impl Widget for BatteryWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let style_color = self.color;
        let style_active = self.active;

        let state = get_battery_state(&self.battery_device);

        if ctx.show_battery_time {
            // Show time estimate text
            let charging = state
                .map(|(_, s)| s == BatteryState::Charging)
                .unwrap_or(false);
            let time_text = get_battery_time_estimate(&self.battery_device, charging)
                .unwrap_or_else(|| "N/A".to_string());
            let text_color = if charging {
                Color::from_rgb(0.3, 0.9, 0.3)
            } else {
                Color::WHITE
            };

            container(
                text(time_text)
                    .font(ctx.font)
                    .size(ctx.font_size * 0.75)
                    .color(text_color)
                    .align_x(alignment::Horizontal::Center)
                    .align_y(alignment::Vertical::Center)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(2)
            .style(move |_theme: &Theme| button_style(style_color, style_active))
            .into()
        } else {
            // Show battery icon + percentage
            match state {
                Some((capacity, bat_state)) => {
                    let charging = bat_state == BatteryState::Charging;
                    let icon = BatteryIconWidget::new(capacity, charging, ctx.blink_on);
                    let label_text = format!("{}%", capacity);
                    let label = text(label_text)
                        .font(ctx.font)
                        .size(ctx.font_size * 0.75)
                        .color(Color::WHITE)
                        .align_x(alignment::Horizontal::Center)
                        .align_y(alignment::Vertical::Center)
                        .width(Length::Fill)
                        .height(Length::Fill);
                    let stacked: Element<'_, Message, Theme, IcedRenderer> = Stack::new()
                        .push(icon)
                        .push(label)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into();
                    container(stacked)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .padding(2)
                        .style(move |_theme: &Theme| button_style(style_color, style_active))
                        .into()
                }
                None => {
                    // Battery read failed
                    container(
                        text("--%")
                            .font(ctx.font)
                            .size(ctx.font_size * 0.75)
                            .color(Color::WHITE)
                            .align_x(alignment::Horizontal::Center)
                            .align_y(alignment::Vertical::Center)
                            .width(Length::Fill)
                            .height(Length::Fill),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(2)
                    .style(move |_theme: &Theme| button_style(style_color, style_active))
                    .into()
                }
            }
        }
    }

    fn update(&mut self) -> bool {
        // Report battery failure via log-once pattern
        let ok = get_battery_state(&self.battery_device).is_some();
        if ok && self.battery_failed {
            eprintln!("Battery sysfs recovered");
            self.battery_failed = false;
        } else if !ok && !self.battery_failed {
            eprintln!("Warning: battery sysfs read failed, showing '--'");
            self.battery_failed = true;
        }
        // Always return true: battery state may change each iteration
        true
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }

    fn handle_event(&mut self, action: WidgetAction) -> Vec<MainLoopAction> {
        match action {
            WidgetAction::Pressed => {
                self.active = true;
                if self.action.is_empty() {
                    vec![MainLoopAction::ShowBatteryTime]
                } else {
                    let mut actions = vec![MainLoopAction::SendKeys(self.action.clone(), true)];
                    actions.push(MainLoopAction::ShowBatteryTime);
                    actions
                }
            }
            WidgetAction::Released => {
                self.active = false;
                if self.action.is_empty() {
                    vec![]
                } else {
                    vec![MainLoopAction::SendKeys(self.action.clone(), false)]
                }
            }
        }
    }
}

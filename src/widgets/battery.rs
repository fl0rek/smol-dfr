use iced_core::alignment;
use iced_core::{Color, Element, Length, Theme};
use iced_widget::{container, text, Stack};
use std::fs;
use std::time::Duration;

use crate::battery_icon_widget::BatteryIconWidget;
use crate::rate_limit::{LogOnce, RateLimitedValue};

use super::{
    button_style, handle_key_action, styled_text_widget_with, IcedRenderer, MainLoopAction,
    Message, RenderContext, Widget, WidgetAction,
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
    let status_path = format!("/sys/class/power_supply/{battery}/status");
    let status = fs::read_to_string(&status_path).unwrap_or_else(|_| "Unknown".to_string());

    let capacity = {
        let base = format!("/sys/class/power_supply/{battery}");
        let from_ratio = |num_file: &str, den_file: &str| -> Option<u32> {
            let num = fs::read_to_string(format!("{base}/{num_file}"))
                .ok()?
                .trim()
                .parse::<f64>()
                .ok()?;
            let den = fs::read_to_string(format!("{base}/{den_file}"))
                .ok()?
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| *v > 0.0)?;
            Some(((num / den) * 100.0).round() as u32)
        };
        from_ratio("charge_now", "charge_full")
            .or_else(|| from_ratio("energy_now", "energy_full"))
            .or_else(|| {
                fs::read_to_string(format!("{base}/capacity"))
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
    let base = format!("/sys/class/power_supply/{battery}");
    let read_val = |file: &str| -> Option<f64> {
        fs::read_to_string(format!("{base}/{file}"))
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
        return Some(format!("{h}h{m:02}m"));
    }

    // 2. Try energy_now / power_now
    let energy_now = read_val("energy_now");
    let energy_full = read_val("energy_full");
    let power_now = read_val("power_now").map(|v| v.abs());

    // 3. Derive energy from charge x voltage if needed
    let (energy_now, energy_full) = if let (Some(en), Some(ef)) = (energy_now, energy_full) {
        (Some(en), Some(ef))
    } else {
        let voltage = read_val("voltage_now")?;
        let cn = read_val("charge_now")?;
        let cf = read_val("charge_full")?;
        (
            Some(cn * voltage / 1_000_000.0),
            Some(cf * voltage / 1_000_000.0),
        )
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
    let m = ((hours - f64::from(h)) * 60.0) as u32;
    Some(format!("{h}h{m:02}m"))
}

/// Cached battery reading: (state, `time_estimate`).
type BatteryReading = (Option<(u32, BatteryState)>, Option<String>);

pub struct BatteryWidget {
    battery_device: String,
    width_fraction: f64,
    action: Vec<input_linux::Key>,
    active: bool,
    color: Option<(f64, f64, f64)>,
    log_once: LogOnce,
    last_capacity: Option<u32>,
    last_state: Option<BatteryState>,
    cached: RateLimitedValue<BatteryReading>,
}

impl BatteryWidget {
    /// Create a new `BatteryWidget`. Returns None if no battery device is found.
    pub fn try_new(
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
            log_once: LogOnce::new(
                "Warning: battery sysfs read failed, showing '--'",
                "Battery sysfs recovered",
            ),
            last_capacity: None,
            last_state: None,
            cached: RateLimitedValue::new((None, None), Duration::from_secs(1)),
        })
    }
}

impl Widget for BatteryWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let style_color = self.color;
        let style_active = self.active;

        let (state, time_estimate) = self.cached.get();
        let state = *state;

        if ctx.show_battery_time {
            // Show time estimate text
            let charging = state.is_some_and(|(_, s)| s == BatteryState::Charging);
            let time_text = time_estimate.clone().unwrap_or_else(|| "N/A".to_string());
            let text_color = if charging {
                Color::from_rgb(0.3, 0.9, 0.3)
            } else {
                Color::WHITE
            };

            styled_text_widget_with(
                time_text,
                ctx,
                style_color,
                style_active,
                ctx.font_size * 0.75,
                text_color,
            )
        } else {
            // Show battery icon + percentage
            match state {
                Some((capacity, bat_state)) => {
                    let charging = bat_state == BatteryState::Charging;
                    let visible = if bat_state == BatteryState::Low {
                        ctx.blink_on
                    } else {
                        true
                    };
                    let icon = BatteryIconWidget::new(capacity, charging, visible);
                    let label_text = format!("{capacity}%");
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
                    styled_text_widget_with(
                        "--%",
                        ctx,
                        style_color,
                        style_active,
                        ctx.font_size * 0.75,
                        Color::WHITE,
                    )
                }
            }
        }
    }

    fn needs_blink(&self) -> bool {
        self.last_state == Some(BatteryState::Low)
    }

    fn update(&mut self) -> bool {
        let dev = self.battery_device.clone();
        self.cached.refresh_if_needed(|| {
            let state = get_battery_state(&dev);
            let charging = state.is_some_and(|(_, s)| s == BatteryState::Charging);
            let time_estimate = get_battery_time_estimate(&dev, charging);
            (state, time_estimate)
        });
        let (state, _) = self.cached.get();
        let state = *state;
        self.log_once.check(state.is_some());
        // Only trigger redraw when capacity or charge state actually changes
        if let Some((capacity, bat_state)) = state {
            let changed =
                self.last_capacity != Some(capacity) || self.last_state != Some(bat_state);
            self.last_capacity = Some(capacity);
            self.last_state = Some(bat_state);
            changed
        } else {
            let was_some = self.last_capacity.is_some();
            self.last_capacity = None;
            self.last_state = None;
            was_some
        }
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }

    fn handle_event(&mut self, action: WidgetAction) -> Vec<MainLoopAction> {
        let mut actions = handle_key_action(&mut self.active, &self.action, action);
        if matches!(action, WidgetAction::Pressed) {
            self.active = true;
            actions.push(MainLoopAction::ShowBatteryTime);
        }
        actions
    }
}

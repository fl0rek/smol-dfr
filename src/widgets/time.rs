use chrono::{
    format::{Item as ChronoItem, Numeric, StrftimeItems},
    Local, Locale,
};
use iced_core::{Element, Theme};

use super::{
    handle_key_action, styled_text_widget, IcedRenderer, MainLoopAction, Message, RenderContext,
    Widget, WidgetAction,
};

pub struct TimeWidget {
    format_items: Vec<ChronoItem<'static>>,
    locale: Locale,
    width_fraction: f64,
    action: Vec<input_linux::Key>,
    active: bool,
    color: Option<(f64, f64, f64)>,
    faster_refresh: bool,
    last_formatted: String,
}

impl TimeWidget {
    pub fn new(
        format: &str,
        locale_str: Option<&str>,
        width_fraction: f64,
        action: Vec<input_linux::Key>,
        color: Option<(f64, f64, f64)>,
    ) -> Self {
        let format_str = if format == "24hr" {
            "%H:%M    %a %-e %b"
        } else if format == "12hr" {
            "%-l:%M %p    %a %-e %b"
        } else {
            format
        };

        let format_items = match StrftimeItems::new(format_str).parse_to_owned() {
            Ok(s) => s,
            Err(e) => panic!(
                "Invalid time format, consult the configuration file for examples of correct ones: {e:?}"
            ),
        };

        let locale = locale_str
            .and_then(|l| Locale::try_from(l).ok())
            .unwrap_or(Locale::POSIX);

        let faster_refresh = format_items.iter().any(|item| {
            matches!(
                item,
                ChronoItem::Numeric(
                    Numeric::Second | Numeric::Nanosecond | Numeric::Timestamp,
                    _
                )
            )
        });

        Self {
            format_items,
            locale,
            width_fraction,
            action,
            active: false,
            color,
            faster_refresh,
            last_formatted: String::new(),
        }
    }
}

impl Widget for TimeWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let label = &self.last_formatted;

        let style_color = self.color;
        let style_active = self.active;

        styled_text_widget(label, ctx, style_color, style_active)
    }

    fn update(&mut self) -> bool {
        let now = Local::now()
            .format_localized_with_items(self.format_items.iter(), self.locale)
            .to_string();
        if now == self.last_formatted {
            false
        } else {
            self.last_formatted = now;
            true
        }
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }

    fn handle_event(&mut self, action: WidgetAction) -> Vec<MainLoopAction> {
        handle_key_action(&mut self.active, &self.action, action)
    }

    fn refresh_interval_ms(&self) -> Option<u32> {
        // Only a seconds-bearing format needs sub-minute updates.
        self.faster_refresh.then_some(1000)
    }
}

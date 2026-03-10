use chrono::{
    format::{Item as ChronoItem, Numeric, StrftimeItems},
    Local, Locale,
};
use iced_core::alignment;
use iced_core::{Color, Element, Length, Theme};
use iced_widget::{container, mouse_area, text};

use super::{button_style, IcedRenderer, MainLoopAction, Message, RenderContext, Widget, WidgetAction};

pub struct TimeWidget {
    format_items: Vec<ChronoItem<'static>>,
    locale: Locale,
    width_fraction: f64,
    action: Vec<input_linux::Key>,
    active: bool,
    color: Option<(f64, f64, f64)>,
    faster_refresh: bool,
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
                ChronoItem::Numeric(Numeric::Second, _)
                    | ChronoItem::Numeric(Numeric::Nanosecond, _)
                    | ChronoItem::Numeric(Numeric::Timestamp, _)
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
        }
    }
}

impl Widget for TimeWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let label = Local::now()
            .format_localized_with_items(self.format_items.iter(), self.locale)
            .to_string();

        let style_color = self.color;
        let style_active = self.active;

        let inner: Element<'_, Message, Theme, IcedRenderer> = container(
            text(label)
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
        .style(move |_theme: &Theme| button_style(style_color, style_active))
        .into();

        if self.action.is_empty() {
            inner
        } else {
            // Wrap in mouse_area -- widget index is set by the renderer
            // when assembling the row, so we use index 0 as placeholder.
            // The actual index mapping happens through the renderer's
            // process_touch_widgets which maps message index to widget index.
            inner
        }
    }

    fn update(&mut self) -> bool {
        // Time always needs redraw; the main loop controls timing.
        false
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }

    fn handle_event(&mut self, action: WidgetAction) -> Vec<MainLoopAction> {
        if self.action.is_empty() {
            return vec![];
        }
        match action {
            WidgetAction::Pressed => {
                self.active = true;
                vec![MainLoopAction::SendKeys(self.action.clone(), true)]
            }
            WidgetAction::Released => {
                self.active = false;
                vec![MainLoopAction::SendKeys(self.action.clone(), false)]
            }
        }
    }

    fn needs_faster_refresh(&self) -> bool {
        self.faster_refresh
    }
}

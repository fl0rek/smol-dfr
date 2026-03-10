use iced_core::alignment;
use iced_core::{Color, ContentFit, Element, Length, Theme};
use iced_widget::{container, svg, text};

use super::{
    button_style, resolve_icon_path, IcedRenderer, MainLoopAction, Message, RenderContext, Widget,
    WidgetAction,
};

/// The kind of static content this button displays.
pub enum StaticVariant {
    Text(String),
    Icon(Option<String>),
    Spacer,
}

pub struct StaticButton {
    variant: StaticVariant,
    width_fraction: f64,
    action: Vec<input_linux::Key>,
    active: bool,
    color: Option<(f64, f64, f64)>,
}

impl StaticButton {
    pub fn new_text(
        label: String,
        action: Vec<input_linux::Key>,
        width_fraction: f64,
        color: Option<(f64, f64, f64)>,
    ) -> Self {
        Self {
            variant: StaticVariant::Text(label),
            width_fraction,
            action,
            active: false,
            color,
        }
    }

    pub fn new_icon(
        name: &str,
        action: Vec<input_linux::Key>,
        width_fraction: f64,
        color: Option<(f64, f64, f64)>,
    ) -> Self {
        let path = resolve_icon_path(name);
        Self {
            variant: StaticVariant::Icon(path),
            width_fraction,
            action,
            active: false,
            color,
        }
    }

    pub fn new_spacer(width_fraction: f64, color: Option<(f64, f64, f64)>) -> Self {
        Self {
            variant: StaticVariant::Spacer,
            width_fraction,
            action: vec![],
            active: false,
            color,
        }
    }
}

impl Widget for StaticButton {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let style_color = self.color;
        let style_active = self.active;

        match &self.variant {
            StaticVariant::Text(label) => container(
                text(label.clone())
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
            .into(),

            StaticVariant::Icon(Some(path)) => {
                let handle = svg::Handle::from_path(path);
                container(
                    svg::Svg::new(handle)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .content_fit(ContentFit::Contain),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(2)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(move |_theme: &Theme| button_style(style_color, style_active))
                .into()
            }

            StaticVariant::Icon(None) => {
                // Icon not found -- render as empty styled container
                container(text("?").color(Color::WHITE))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(2)
                    .style(move |_theme: &Theme| button_style(style_color, style_active))
                    .into()
            }

            StaticVariant::Spacer => container(text(""))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_theme: &Theme| button_style(style_color, style_active))
                .into(),
        }
    }

    fn update(&mut self) -> bool {
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
}

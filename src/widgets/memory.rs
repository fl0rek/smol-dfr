use iced_core::alignment;
use iced_core::{Color, Element, Length, Theme};
use iced_widget::{container, text};

use crate::memory_graph::{get_memory_usage, MemoryHistory};
use crate::memory_graph_widget::MemoryGraphWidget;

use super::{button_style, IcedRenderer, Message, RenderContext, Widget};

pub struct MemoryWidget {
    history: MemoryHistory,
    width_fraction: f64,
    color: Option<(f64, f64, f64)>,
}

impl MemoryWidget {
    pub fn new(
        sample_interval_ms: u32,
        graph_window_s: u32,
        width_fraction: f64,
        color: Option<(f64, f64, f64)>,
    ) -> Self {
        Self {
            history: MemoryHistory::new(sample_interval_ms, graph_window_s),
            width_fraction,
            color,
        }
    }
}

impl Widget for MemoryWidget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer> {
        let style_color = self.color;
        let style_active = false;

        let samples = self.history.samples();
        if samples.is_empty() {
            // No samples yet, show percentage text
            let usage = get_memory_usage();
            container(
                text(format!("{}%", usage))
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
            .into()
        } else {
            // Render memory graph
            let graph = MemoryGraphWidget::new(
                samples.iter().copied().collect(),
                self.history.max_samples(),
            );
            container(graph)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(2)
                .style(move |_theme: &Theme| button_style(style_color, style_active))
                .into()
        }
    }

    fn update(&mut self) -> bool {
        self.history.maybe_sample()
    }

    fn width_fraction(&self) -> f64 {
        self.width_fraction
    }

    fn refresh_interval_ms(&self) -> Option<u32> {
        // The graph samples on a fixed schedule; without this the main loop
        // would sleep up to TIMEOUT_MS and starve it.
        Some(self.history.sample_interval_ms())
    }
}

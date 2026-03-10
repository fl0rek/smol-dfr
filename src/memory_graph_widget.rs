use iced_core::layout::{Layout, Limits, Node};
use iced_core::mouse;
use iced_core::renderer;
use iced_core::widget::Tree;
use iced_core::{Background, Color, Element, Length, Rectangle, Size, Theme, Widget};

pub struct MemoryGraphWidget {
    samples: Vec<u32>,
    max_columns: usize,
}

impl MemoryGraphWidget {
    pub fn new(samples: Vec<u32>, max_columns: usize) -> Self {
        Self {
            samples,
            max_columns,
        }
    }
}

fn value_to_color(value: u32) -> Color {
    let v = (value as f32).clamp(0.0, 100.0) / 100.0;
    // 0% = green (0,1,0) -> 50% = yellow (1,1,0) -> 100% = red (1,0,0)
    let r = if v <= 0.5 { v * 2.0 } else { 1.0 };
    let g = if v <= 0.5 { 1.0 } else { 1.0 - (v - 0.5) * 2.0 };
    // tiny-skia renders fill_quad with BGRA byte order internally,
    // so swap R and B to get correct output after XRGB8888 conversion
    Color::from_rgb(0.0, g, r)
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for MemoryGraphWidget
where
    Renderer: renderer::Renderer,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&self, _tree: &mut Tree, _renderer: &Renderer, limits: &Limits) -> Node {
        Node::new(limits.max())
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if self.max_columns == 0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        let col_width = bounds.width / self.max_columns as f32;
        let n = self.samples.len();

        // Right-aligned: empty space on left when history not full
        let start_x = bounds.x + (self.max_columns - n) as f32 * col_width;

        for (i, &value) in self.samples.iter().enumerate() {
            let bar_height = (value as f32 / 100.0) * bounds.height;
            let x = start_x + i as f32 * col_width;
            let y = bounds.y + bounds.height - bar_height;

            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x,
                        y,
                        width: col_width,
                        height: bar_height,
                    },
                    border: iced_core::Border::default(),
                    shadow: iced_core::Shadow::default(),
                },
                Background::Color(value_to_color(value)),
            );
        }

        // Notches at 25%, 50%, 75% on both sides
        let notch_color = Color::from_rgba(0.5, 0.5, 0.5, 0.8);
        let notch_w = 6.0;
        for pct in [25.0, 50.0, 75.0] {
            let y = bounds.y + bounds.height * (1.0 - pct / 100.0);
            for x in [bounds.x, bounds.x + bounds.width - notch_w] {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle {
                            x,
                            y,
                            width: notch_w,
                            height: 2.0,
                        },
                        border: iced_core::Border::default(),
                        shadow: iced_core::Shadow::default(),
                    },
                    Background::Color(notch_color),
                );
            }
        }
    }
}

impl<'a, Message, Renderer> From<MemoryGraphWidget> for Element<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
    Message: 'a,
{
    fn from(widget: MemoryGraphWidget) -> Self {
        Element::new(widget)
    }
}

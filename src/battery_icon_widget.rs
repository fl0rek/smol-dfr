use iced_core::layout::{Layout, Limits, Node};
use iced_core::mouse;
use iced_core::renderer;
use iced_core::widget::Tree;
use iced_core::{Background, Color, Element, Length, Rectangle, Size, Theme, Widget};

pub struct BatteryIconWidget {
    capacity: u32,
    charging: bool,
    visible: bool,
}

impl BatteryIconWidget {
    pub fn new(capacity: u32, charging: bool, visible: bool) -> Self {
        Self {
            capacity,
            charging,
            visible,
        }
    }

    fn icon_color(&self) -> Color {
        if self.charging {
            // Green — no R/B swap needed (R=B=0)
            Color::from_rgb(0.0, 1.0, 0.0)
        } else if self.capacity < 15 {
            // Red — R/B swapped for fill_quad BGRA: desired (1,0,0) → (0,0,1)
            Color::from_rgb(0.0, 0.0, 1.0)
        } else if self.capacity < 30 {
            // Yellow — R/B swapped: desired (1,1,0) → (0,1,1)
            Color::from_rgb(0.0, 1.0, 1.0)
        } else {
            Color::from_rgb(1.0, 1.0, 1.0)
        }
    }
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for BatteryIconWidget
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
        if !self.visible {
            return;
        }

        let bounds = layout.bounds();
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        let color = self.icon_color();
        let outline = 2.0;
        let nub_width = bounds.width * 0.06;
        let nub_height = bounds.height * 0.35;
        let v_inset = bounds.height * 0.08;

        // Body area (excluding nub)
        let body_x = bounds.x;
        let body_y = bounds.y + v_inset;
        let body_w = bounds.width - nub_width;
        let body_h = bounds.height - 2.0 * v_inset;

        // 1. Body outline
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: body_x,
                    y: body_y,
                    width: body_w,
                    height: body_h,
                },
                border: iced_core::Border::default(),
                shadow: iced_core::Shadow::default(),
            },
            Background::Color(color),
        );

        // 2. Black interior (hollow out)
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: body_x + outline,
                    y: body_y + outline,
                    width: body_w - 2.0 * outline,
                    height: body_h - 2.0 * outline,
                },
                border: iced_core::Border::default(),
                shadow: iced_core::Shadow::default(),
            },
            Background::Color(Color::BLACK),
        );

        // 3. Nub on the right
        let nub_x = body_x + body_w;
        let nub_y = bounds.y + (bounds.height - nub_height) / 2.0;
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: nub_x,
                    y: nub_y,
                    width: nub_width,
                    height: nub_height,
                },
                border: iced_core::Border::default(),
                shadow: iced_core::Shadow::default(),
            },
            Background::Color(color),
        );

        // 4. Fill bar (semi-transparent for text readability)
        let interior_w = body_w - 2.0 * outline;
        let fill_w = interior_w * (self.capacity as f32 / 100.0);
        let fill_color = Color { a: 0.5, ..color };
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: body_x + outline,
                    y: body_y + outline,
                    width: fill_w,
                    height: body_h - 2.0 * outline,
                },
                border: iced_core::Border::default(),
                shadow: iced_core::Shadow::default(),
            },
            Background::Color(fill_color),
        );
    }
}

impl<'a, Message, Renderer> From<BatteryIconWidget> for Element<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
    Message: 'a,
{
    fn from(widget: BatteryIconWidget) -> Self {
        Element::new(widget)
    }
}

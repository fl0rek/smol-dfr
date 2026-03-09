use iced_core::alignment;
use iced_core::layout::{Layout, Limits};
use iced_core::mouse;
use iced_core::renderer;
use iced_core::widget::Tree;
use iced_core::Renderer as _;
use iced_core::{
    Background, Color, Element, Font, Length, Pixels, Rectangle, Size, Theme,
};
use iced_graphics::Viewport;
use iced_widget::{container, row, text};
use tiny_skia::Pixmap;

type IcedRenderer = iced_tiny_skia::Renderer;

pub struct TouchbarRenderer {
    renderer: IcedRenderer,
    /// Long axis (≈2170)
    logical_width: u32,
    /// Visible short axis from DRM mode (≈60) — used for widget layout
    visible_height: u32,
    /// Framebuffer short axis (64) — used for pixmap/rotation buffer size
    fb_height: u32,
}

#[derive(Debug, Clone)]
pub enum Message {}

impl TouchbarRenderer {
    pub fn new(logical_width: u32, visible_height: u32, fb_height: u32) -> Self {
        let renderer = IcedRenderer::new(Font::DEFAULT, Pixels(24.0));
        Self {
            renderer,
            logical_width,
            visible_height,
            fb_height,
        }
    }

    /// Build the POC widget tree, render it to a pixmap, and return the
    /// rotated XRGB8888 buffer ready for the DRM framebuffer.
    ///
    /// Layout, draw, and pixel flush all happen in one call so that the widget
    /// Tree (which owns the Paragraph state) stays alive while the renderer
    /// flushes text via Weak references.
    pub fn render_to_buffer(&mut self) -> Vec<u8> {
        self.renderer.clear();

        let labels = [
            "esc", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
        ];

        let element = build_button_row(&labels);
        let mut tree = Tree::new(element.as_widget());
        element.as_widget().diff(&mut tree);

        let w = self.logical_width;
        // Layout uses visible_height (≈60) so content fits in the visible area.
        // Cairo does the same: it draws in (width x height) logical space where
        // height = mode height ≈ 60, not the framebuffer's 64.
        let vis_h = self.visible_height;
        // The pixmap and rotation output use fb_height (64) to match the DRM
        // dumb buffer dimensions.
        let fb_h = self.fb_height;

        let limits = Limits::new(
            Size::ZERO,
            Size::new(w as f32, vis_h as f32),
        );

        let node = element.as_widget().layout(&mut tree, &self.renderer, &limits);
        let layout = Layout::new(&node);

        let theme = Theme::KanagawaDragon;
        let style = renderer::Style {
            text_color: Color::WHITE,
        };
        let cursor = mouse::Cursor::Unavailable;
        // Viewport covers only the visible area so widgets are clipped correctly
        let viewport_rect = Rectangle {
            x: 0.0,
            y: 0.0,
            width: w as f32,
            height: vis_h as f32,
        };

        element.as_widget().draw(
            &tree,
            &mut self.renderer,
            &theme,
            &style,
            layout,
            cursor,
            &viewport_rect,
        );

        // Flush to pixmap while tree (and its Paragraphs) is still alive.
        // Pixmap uses fb_height so the rotation output fills the full
        // framebuffer stride. Content only occupies the first vis_h rows;
        // the remaining (fb_h - vis_h) rows stay black.
        let mut pixmap = Pixmap::new(w, fb_h).expect("Failed to create pixmap");
        let mut clip_mask = tiny_skia::Mask::new(w, fb_h).expect("Failed to create clip mask");
        let viewport = Viewport::with_physical_size(Size::new(w, fb_h), 1.0);
        // Damage only the visible area
        let damage = [viewport_rect];

        self.renderer.draw(
            &mut pixmap.as_mut(),
            &mut clip_mask,
            &viewport,
            &damage,
            Color::BLACK,
            &[] as &[&str],
        );

        // Rotate 90 CW: landscape pixmap (w, fb_h) -> portrait buffer (fb_h, w)
        //
        // Cairo's rotation: c.translate(height, 0); c.rotate(90deg)
        // maps logical (lx, ly) -> framebuffer (height - ly, lx)
        // where height = visible_height ≈ 60.
        //
        // Our CW rotation maps (sx, sy) -> (fb_h - 1 - sy, sx).
        // Content at sy=0 maps to dx=fb_h-1=63, sy=vis_h-1=59 maps to dx=4.
        // That leaves dx=0..3 black, matching Cairo which leaves fb X>height black.
        //
        // To match Cairo exactly (content at dx=0..height-1, black at dx>=height):
        //   dx = visible_height - 1 - sy   (for sy < vis_h, skip sy >= vis_h)
        let dst_w = fb_h as usize;
        let dst_h = w as usize;
        let src_data = pixmap.data();
        let mut rotated = vec![0u8; dst_w * dst_h * 4];

        for sy in 0..vis_h as usize {
            for sx in 0..w as usize {
                let src_idx = (sy * w as usize + sx) * 4;
                let dx = vis_h as usize - 1 - sy;
                let dy = sx;
                let dst_idx = (dy * dst_w + dx) * 4;

                // tiny-skia stores premultiplied RGBA; DRM expects XRGB8888
                let r = src_data[src_idx];
                let g = src_data[src_idx + 1];
                let b = src_data[src_idx + 2];
                let a = src_data[src_idx + 3];

                let (r, g, b) = if a == 0 {
                    (0, 0, 0)
                } else if a == 255 {
                    (r, g, b)
                } else {
                    let a_f = a as f32 / 255.0;
                    (
                        (r as f32 / a_f).min(255.0) as u8,
                        (g as f32 / a_f).min(255.0) as u8,
                        (b as f32 / a_f).min(255.0) as u8,
                    )
                };

                // XRGB8888 little-endian: byte order is B, G, R, X
                rotated[dst_idx] = b;
                rotated[dst_idx + 1] = g;
                rotated[dst_idx + 2] = r;
                rotated[dst_idx + 3] = 0xFF;
            }
        }

        rotated
    }
}

fn build_button_row<'a>(labels: &'a [&'a str]) -> Element<'a, Message, Theme, IcedRenderer> {
    let spacing = 4;
    let padding = 2;

    let buttons: Vec<Element<'_, Message, Theme, IcedRenderer>> = labels
        .iter()
        .map(|label| {
            container(
                text(label.to_string())
                    .size(20)
                    .color(Color::WHITE)
                    .align_x(alignment::Horizontal::Center)
                    .align_y(alignment::Vertical::Center)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(padding)
            .style(|_theme: &Theme| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.2, 0.2, 0.2))),
                border: iced_core::Border {
                    radius: 8.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
        })
        .collect();

    container(
        row(buttons)
            .spacing(spacing)
            .height(Length::Fill)
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(2)
    .into()
}

use iced_core::clipboard;
use iced_core::font::{Family, Stretch, Style, Weight};
use iced_core::layout::{Layout, Limits};
use iced_core::mouse;
use iced_core::renderer;
use iced_core::widget::Tree;
use iced_core::Length;
use iced_core::Renderer as _;
use iced_core::{Color, Element, Font, Pixels, Rectangle, Shell, Size, Theme};
use iced_graphics::Viewport;
use iced_widget::{container, mouse_area, row};
use tiny_skia::Pixmap;

use crate::widgets::{Message as WidgetMessage, RenderContext, Widget};

type IcedRenderer = iced_tiny_skia::Renderer;

pub struct TouchbarRenderer {
    renderer: IcedRenderer,
    /// Long axis (~2170)
    logical_width: u32,
    /// Visible short axis from DRM mode (~60) -- used for widget layout
    visible_height: u32,
    /// Framebuffer short axis (64) -- used for pixmap/rotation buffer size
    fb_height: u32,
    /// Persistent widget tree for the Widget rendering path
    widget_tree: Option<Tree>,
    font: Font,
    font_size: f32,
    /// Reusable pixmap buffer (logical_width × fb_height)
    pixmap: Pixmap,
    /// Reusable clip mask (logical_width × fb_height)
    clip_mask: tiny_skia::Mask,
    /// Reusable rotation output buffer (fb_height × logical_width × 4 bytes)
    rotated_buf: Vec<u8>,
}

impl TouchbarRenderer {
    pub fn new(
        logical_width: u32,
        visible_height: u32,
        fb_height: u32,
        font_family: &str,
        font_size: f32,
        font_bold: bool,
        font_italic: bool,
    ) -> Self {
        let family = if font_family.is_empty() {
            Family::SansSerif
        } else {
            Family::Name(font_family.to_string().leak())
        };
        let font = Font {
            family,
            weight: if font_bold {
                Weight::Bold
            } else {
                Weight::Normal
            },
            style: if font_italic {
                Style::Italic
            } else {
                Style::Normal
            },
            stretch: Stretch::Normal,
        };
        let renderer = IcedRenderer::new(font, Pixels(font_size));
        let pixmap = Pixmap::new(logical_width, fb_height).expect("Failed to create pixmap");
        let clip_mask =
            tiny_skia::Mask::new(logical_width, fb_height).expect("Failed to create clip mask");
        let rotated_buf = vec![0u8; fb_height as usize * logical_width as usize * 4];
        Self {
            renderer,
            logical_width,
            visible_height,
            fb_height,
            widget_tree: None,
            font,
            font_size,
            pixmap,
            clip_mask,
            rotated_buf,
        }
    }

    pub fn font(&self) -> Font {
        self.font
    }

    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Sync a persistent tree with a widget structure.
    fn sync_tree_slot<M: 'static>(
        slot: &mut Option<Tree>,
        element: &Element<'_, M, Theme, IcedRenderer>,
    ) {
        match slot {
            Some(tree) => element.as_widget().diff(tree),
            slot @ None => *slot = Some(Tree::new(element.as_widget())),
        }
    }

    /// Shared layout+draw+rotate pipeline. Reuses internal buffers to avoid
    /// per-frame allocations (~1.1 MB saved per render).
    fn render_element<M: 'static>(
        &mut self,
        element: &Element<'_, M, Theme, IcedRenderer>,
        tree: &mut Tree,
    ) -> &[u8] {
        let w = self.logical_width;
        let vis_h = self.visible_height;
        let fb_h = self.fb_height;

        let limits = Limits::new(Size::ZERO, Size::new(w as f32, vis_h as f32));
        let node = element.as_widget().layout(tree, &self.renderer, &limits);
        let layout = Layout::new(&node);

        let theme = Theme::KanagawaDragon;
        let style = renderer::Style {
            text_color: Color::WHITE,
        };
        let cursor = mouse::Cursor::Unavailable;
        let viewport_rect = Rectangle {
            x: 0.0,
            y: 0.0,
            width: w as f32,
            height: vis_h as f32,
        };

        element.as_widget().draw(
            tree,
            &mut self.renderer,
            &theme,
            &style,
            layout,
            cursor,
            &viewport_rect,
        );

        // Clear reusable pixmap and flush renderer into it
        self.pixmap.data_mut().fill(0);
        let viewport = Viewport::with_physical_size(Size::new(w, fb_h), 1.0);
        let damage = [viewport_rect];

        self.renderer.draw(
            &mut self.pixmap.as_mut(),
            &mut self.clip_mask,
            &viewport,
            &damage,
            Color::BLACK,
            &[] as &[&str],
        );

        #[cfg(feature = "screenshot")]
        {
            match self.pixmap.encode_png() {
                Ok(png_data) => {
                    if let Err(e) = std::fs::write("/tmp/smol-dfr-screenshot.png", &png_data) {
                        eprintln!("screenshot: failed to write PNG: {e}");
                    }
                }
                Err(e) => eprintln!("screenshot: failed to encode PNG: {e}"),
            }
        }

        // Rotate 90 CW: landscape pixmap (w, fb_h) -> portrait buffer (fb_h, w)
        // Uses destination-row-major iteration for cache-friendly sequential writes.
        rotate_and_convert_into(
            self.pixmap.data(),
            w as usize,
            vis_h as usize,
            fb_h as usize,
            w as usize,
            &mut self.rotated_buf,
        );
        &self.rotated_buf
    }

    /// Render a list of Widget trait objects to a rotated XRGB8888 buffer.
    /// Returns a slice into the internal rotation buffer (zero allocations).
    pub fn render_widgets(&mut self, widgets: &[Box<dyn Widget>], ctx: &RenderContext) -> &[u8] {
        self.renderer.clear();

        let element = build_widget_row(widgets, ctx);
        Self::sync_tree_slot(&mut self.widget_tree, &element);
        let mut tree = self.widget_tree.take().unwrap();

        self.render_element(&element, &mut tree);

        self.widget_tree = Some(tree);
        &self.rotated_buf
    }

    /// Process a touch event through the Widget rendering path.
    pub fn process_touch_widgets(
        &mut self,
        iced_event: iced_core::Event,
        cursor: mouse::Cursor,
        widgets: &[Box<dyn Widget>],
        ctx: &RenderContext,
    ) -> Vec<WidgetMessage> {
        let mut element = build_widget_row(widgets, ctx);
        Self::sync_tree_slot(&mut self.widget_tree, &element);
        let mut tree = self.widget_tree.take().unwrap();

        let w = self.logical_width;
        let vis_h = self.visible_height;

        let limits = Limits::new(Size::ZERO, Size::new(w as f32, vis_h as f32));
        let node = element
            .as_widget()
            .layout(&mut tree, &self.renderer, &limits);
        let layout = Layout::new(&node);

        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        let viewport = Rectangle {
            x: 0.0,
            y: 0.0,
            width: w as f32,
            height: vis_h as f32,
        };

        element.as_widget_mut().on_event(
            &mut tree,
            iced_event,
            layout,
            cursor,
            &self.renderer,
            &mut clipboard::Null,
            &mut shell,
            &viewport,
        );

        self.widget_tree = Some(tree);
        messages
    }
}

/// Translate a libinput touch event into an iced Event + Cursor.
pub fn translate_touch(
    te: &input::event::touch::TouchEvent,
    touch_positions: &mut std::collections::HashMap<u32, iced_core::Point>,
    width: u16,
    height: u16,
) -> Option<(iced_core::Event, mouse::Cursor)> {
    use input::event::touch::{TouchEventPosition, TouchEventSlot};
    match te {
        input::event::touch::TouchEvent::Down(dn) => {
            let pos = iced_core::Point::new(
                dn.x_transformed(width as u32) as f32,
                dn.y_transformed(height as u32) as f32,
            );
            touch_positions.insert(dn.seat_slot(), pos);
            Some((
                iced_core::Event::Touch(iced_core::touch::Event::FingerPressed {
                    id: iced_core::touch::Finger(dn.seat_slot() as u64),
                    position: pos,
                }),
                mouse::Cursor::Available(pos),
            ))
        }
        input::event::touch::TouchEvent::Motion(mv) => {
            let pos = iced_core::Point::new(
                mv.x_transformed(width as u32) as f32,
                mv.y_transformed(height as u32) as f32,
            );
            touch_positions.insert(mv.seat_slot(), pos);
            Some((
                iced_core::Event::Touch(iced_core::touch::Event::FingerMoved {
                    id: iced_core::touch::Finger(mv.seat_slot() as u64),
                    position: pos,
                }),
                mouse::Cursor::Available(pos),
            ))
        }
        input::event::touch::TouchEvent::Up(up) => {
            let pos = touch_positions
                .remove(&up.seat_slot())
                .unwrap_or(iced_core::Point::ORIGIN);
            Some((
                iced_core::Event::Touch(iced_core::touch::Event::FingerLifted {
                    id: iced_core::touch::Finger(up.seat_slot() as u64),
                    position: pos,
                }),
                mouse::Cursor::Available(pos),
            ))
        }
        _ => None,
    }
}

/// Rotate 90 degrees clockwise and convert premultiplied RGBA to BGRX (XRGB8888),
/// writing into a caller-provided buffer to avoid per-frame allocation.
///
/// Iterates in destination-row-major order for cache-friendly sequential writes.
/// Source pixels are premultiplied alpha; this function unpacks them before storing.
///
/// Parameters:
/// - `src_data`: premultiplied RGBA pixel data (row-major, `src_w` pixels wide)
/// - `src_w`: source width (logical_width, the long axis)
/// - `src_h`: source height (visible rows to process, may be < pixmap height)
/// - `dst_w`: destination width after rotation (fb_height)
/// - `dst_h`: destination height after rotation (logical_width)
/// - `dst`: output buffer, must be at least `dst_w * dst_h * 4` bytes
fn rotate_and_convert_into(
    src_data: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    dst: &mut [u8],
) {
    // Clear buffer (padding rows beyond src_h need to be black)
    dst[..dst_w * dst_h * 4].fill(0);

    for dy in 0..dst_h {
        for dx in 0..dst_w {
            let dst_idx = (dy * dst_w + dx) * 4;

            // Inverse of 90 CW rotation: src_x = dy, src_y = (dst_w - 1 - dx)
            let sy = dst_w - 1 - dx;
            if sy >= src_h {
                // Beyond visible rows -- already zeroed
                continue;
            }
            let sx = dy;

            let src_idx = (sy * src_w + sx) * 4;
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

            dst[dst_idx] = b;
            dst[dst_idx + 1] = g;
            dst[dst_idx + 2] = r;
            dst[dst_idx + 3] = 0xFF;
        }
    }
}

/// Allocating version for tests.
#[cfg(test)]
fn rotate_and_convert(
    src_data: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<u8> {
    let mut dst = vec![0u8; dst_w * dst_h * 4];
    rotate_and_convert_into(src_data, src_w, src_h, dst_w, dst_h, &mut dst);
    dst
}

/// Build an iced Element row from Widget trait objects.
fn build_widget_row<'a>(
    widgets: &'a [Box<dyn Widget>],
    ctx: &'a RenderContext,
) -> Element<'a, WidgetMessage, Theme, IcedRenderer> {
    let children: Vec<Element<'a, WidgetMessage, Theme, IcedRenderer>> = widgets
        .iter()
        .enumerate()
        .map(|(idx, w)| {
            let portion = (w.width_fraction() * 1000.0).round() as u16;
            let inner = w.render(ctx);
            // Wrap in mouse_area for generic press/release (StaticButton, TimeWidget, etc.)
            let wrapped: Element<'a, WidgetMessage, Theme, IcedRenderer> = mouse_area(inner)
                .on_press(WidgetMessage::WidgetPressed(idx))
                .on_release(WidgetMessage::WidgetReleased(idx))
                .on_exit(WidgetMessage::WidgetReleased(idx))
                .into();
            container(wrapped)
                .width(Length::FillPortion(portion))
                .height(Length::Fill)
                .into()
        })
        .collect();

    container(
        row(children)
            .spacing(4)
            .height(Length::Fill)
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(2)
    .into()
}

#[cfg(test)]
mod tests {
    use super::rotate_and_convert;

    /// Helper: create a premultiplied RGBA pixel
    fn px(r: u8, g: u8, b: u8, a: u8) -> [u8; 4] {
        [r, g, b, a]
    }

    /// Build a flat RGBA buffer from a 2D array of pixels (row-major).
    fn build_src(rows: &[&[[u8; 4]]]) -> Vec<u8> {
        rows.iter()
            .flat_map(|row| row.iter().flat_map(|p| p.iter().copied()))
            .collect()
    }

    /// Read a BGRX pixel from output buffer at (x, y) with given stride.
    fn read_dst(buf: &[u8], x: usize, y: usize, stride: usize) -> (u8, u8, u8, u8) {
        let idx = (y * stride + x) * 4;
        (buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]) // B, G, R, X
    }

    #[test]
    fn test_4x3_rotation_layout() {
        // Source: 4 wide x 3 tall, each pixel unique for tracking
        // Pixel values are fully opaque (a=255) so RGB passes through
        let row0: &[[u8; 4]] = &[
            px(10, 20, 30, 255),
            px(11, 21, 31, 255),
            px(12, 22, 32, 255),
            px(13, 23, 33, 255),
        ];
        let row1: &[[u8; 4]] = &[
            px(40, 50, 60, 255),
            px(41, 51, 61, 255),
            px(42, 52, 62, 255),
            px(43, 53, 63, 255),
        ];
        let row2: &[[u8; 4]] = &[
            px(70, 80, 90, 255),
            px(71, 81, 91, 255),
            px(72, 82, 92, 255),
            px(73, 83, 93, 255),
        ];
        let src = build_src(&[row0, row1, row2]);

        let src_w = 4;
        let src_h = 3;
        let dst_w = 3; // fb_height = src_h
        let dst_h = 4; // logical_width = src_w

        let out = rotate_and_convert(&src, src_w, src_h, dst_w, dst_h);

        // Output is dst_w * dst_h * 4 bytes
        assert_eq!(out.len(), dst_w * dst_h * 4);

        // 90 CW rotation: src(sx, sy) -> dst(src_h - 1 - sy, sx)
        // So src(0,0) -> dst(2, 0), src(1,0) -> dst(2,1), etc.
        // dst(dx, dy): sx = dy, sy = dst_w - 1 - dx

        // Check src(0,0) = (10,20,30) -> dst(2,0) in BGRX
        let (b, g, r, x) = read_dst(&out, 2, 0, dst_w);
        assert_eq!((r, g, b, x), (10, 20, 30, 0xFF));

        // Check src(3,2) = (73,83,93) -> dst(0, 3)
        let (b, g, r, x) = read_dst(&out, 0, 3, dst_w);
        assert_eq!((r, g, b, x), (73, 83, 93, 0xFF));

        // Check src(1,1) = (41,51,61) -> dst(1, 1)
        let (b, g, r, x) = read_dst(&out, 1, 1, dst_w);
        assert_eq!((r, g, b, x), (41, 51, 61, 0xFF));
    }

    #[test]
    fn test_transparent_pixels_produce_black() {
        let row: &[[u8; 4]] = &[px(0, 0, 0, 0), px(128, 64, 32, 0)];
        let src = build_src(&[row]);

        let out = rotate_and_convert(&src, 2, 1, 1, 2);
        // dst(0,0): sy = 0, sx = 0 -> src(0,0) transparent
        let (b, g, r, x) = read_dst(&out, 0, 0, 1);
        assert_eq!((r, g, b, x), (0, 0, 0, 0xFF));

        // dst(0,1): sy = 0, sx = 1 -> src(1,0) transparent
        let (b, g, r, x) = read_dst(&out, 0, 1, 1);
        assert_eq!((r, g, b, x), (0, 0, 0, 0xFF));
    }

    #[test]
    fn test_opaque_pixels_passthrough() {
        let row: &[[u8; 4]] = &[px(200, 100, 50, 255)];
        let src = build_src(&[row]);

        let out = rotate_and_convert(&src, 1, 1, 1, 1);
        let (b, g, r, x) = read_dst(&out, 0, 0, 1);
        assert_eq!((r, g, b, x), (200, 100, 50, 0xFF));
    }

    #[test]
    fn test_semitransparent_unpremultiply() {
        // Premultiplied: r=128, g=0, b=0, a=128
        // Unpacked: r = 128 / (128/255.0) ~= 254..255 due to float rounding
        // The division 128.0 / (128.0/255.0) can yield 254 or 255 depending on
        // floating-point intermediates. We accept either.
        let row: &[[u8; 4]] = &[px(128, 0, 0, 128)];
        let src = build_src(&[row]);

        let out = rotate_and_convert(&src, 1, 1, 1, 1);
        let (b, g, r, x) = read_dst(&out, 0, 0, 1);
        assert!(r >= 254, "r should be ~255 after unpremultiply, got {r}");
        assert_eq!((g, b, x), (0, 0, 0xFF));

        // Also test a case where unpremultiply clearly changes the value:
        // Premultiplied: r=64, g=32, b=16, a=128 -> unpacked: r=128, g=64, b=32
        let row2: &[[u8; 4]] = &[px(64, 32, 16, 128)];
        let src2 = build_src(&[row2]);
        let out2 = rotate_and_convert(&src2, 1, 1, 1, 1);
        let (b2, g2, r2, x2) = read_dst(&out2, 0, 0, 1);
        // Allow +/-1 for float rounding
        assert!((127..=129).contains(&r2), "r should be ~128, got {r2}");
        assert!((63..=65).contains(&g2), "g should be ~64, got {g2}");
        assert!((31..=33).contains(&b2), "b should be ~32, got {b2}");
        assert_eq!(x2, 0xFF);
    }

    #[test]
    fn test_output_dimensions_transposed() {
        // 5 wide x 2 tall -> dst should be (2 wide x 5 tall)
        let row0: &[[u8; 4]] = &[px(0, 0, 0, 255); 5];
        let row1: &[[u8; 4]] = &[px(0, 0, 0, 255); 5];
        let src = build_src(&[row0, row1]);

        let dst_w = 2;
        let dst_h = 5;
        let out = rotate_and_convert(&src, 5, 2, dst_w, dst_h);
        assert_eq!(out.len(), dst_w * dst_h * 4);
    }

    #[test]
    fn test_padding_rows_stay_black() {
        // src_h < dst_w means some destination pixels map to rows beyond visible area
        // src: 2 wide x 1 tall, dst_w = 3 (fb_height > vis_h), dst_h = 2
        let row: &[[u8; 4]] = &[px(255, 128, 64, 255), px(200, 100, 50, 255)];
        // Need source buffer to be 2 wide * 3 tall (dst_w rows) to avoid OOB
        // Actually src_data comes from pixmap which is src_w * fb_h, so pad with zeros
        let mut src = build_src(&[row]);
        // Add 2 more rows of zeros (for fb_h=3 total)
        src.extend(vec![0u8; 2 * 4 * 2]);

        let out = rotate_and_convert(&src, 2, 1, 3, 2);
        // dst(2, 0): sy = 3-1-2 = 0, sx = 0 -> src(0,0) visible
        let (b, g, r, x) = read_dst(&out, 2, 0, 3);
        assert_eq!((r, g, b, x), (255, 128, 64, 0xFF));

        // dst(1, 0): sy = 3-1-1 = 1, sx = 0 -> sy=1 >= src_h=1, padding -> black/zero
        let (b, g, r, x) = read_dst(&out, 1, 0, 3);
        assert_eq!((r, g, b, x), (0, 0, 0, 0));

        // dst(0, 0): sy = 3-1-0 = 2, sx = 0 -> sy=2 >= src_h=1, padding -> black/zero
        let (b, g, r, x) = read_dst(&out, 0, 0, 3);
        assert_eq!((r, g, b, x), (0, 0, 0, 0));
    }
}

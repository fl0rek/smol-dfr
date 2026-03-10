use iced_core::clipboard;
use iced_core::layout::{Layout, Limits};
use iced_core::mouse;
use iced_core::renderer;
use iced_core::widget::Tree;
use iced_core::Renderer as _;
use iced_core::font::{Family, Stretch, Style, Weight};
use iced_core::{
    Color, Element, Font, Pixels, Rectangle, Shell, Size, Theme,
};
use iced_graphics::Viewport;
use iced_widget::{container, mouse_area, row};
use iced_core::Length;
use tiny_skia::Pixmap;

use crate::widgets::{RenderContext, Widget, Message as WidgetMessage};

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
            weight: if font_bold { Weight::Bold } else { Weight::Normal },
            style: if font_italic { Style::Italic } else { Style::Normal },
            stretch: Stretch::Normal,
        };
        let renderer = IcedRenderer::new(font, Pixels(font_size));
        Self {
            renderer,
            logical_width,
            visible_height,
            fb_height,
            widget_tree: None,
            font,
            font_size,
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

    /// Shared layout+draw+rotate pipeline.
    fn render_element<M: 'static>(
        &mut self,
        element: &Element<'_, M, Theme, IcedRenderer>,
        tree: &mut Tree,
    ) -> Vec<u8> {
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

        // Flush to pixmap
        let mut pixmap = Pixmap::new(w, fb_h).expect("Failed to create pixmap");
        let mut clip_mask =
            tiny_skia::Mask::new(w, fb_h).expect("Failed to create clip mask");
        let viewport = Viewport::with_physical_size(Size::new(w, fb_h), 1.0);
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

                rotated[dst_idx] = b;
                rotated[dst_idx + 1] = g;
                rotated[dst_idx + 2] = r;
                rotated[dst_idx + 3] = 0xFF;
            }
        }

        rotated
    }

    /// Render a list of Widget trait objects to a rotated XRGB8888 buffer.
    pub fn render_widgets(
        &mut self,
        widgets: &[Box<dyn Widget>],
        ctx: &RenderContext,
    ) -> Vec<u8> {
        self.renderer.clear();

        let element = build_widget_row(widgets, ctx);
        Self::sync_tree_slot(&mut self.widget_tree, &element);
        let mut tree = self.widget_tree.take().unwrap();

        let rotated = self.render_element(&element, &mut tree);

        self.widget_tree = Some(tree);
        rotated
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
        let node = element.as_widget().layout(&mut tree, &self.renderer, &limits);
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

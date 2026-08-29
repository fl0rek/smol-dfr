use std::sync::Mutex;

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

use crate::rotate::rotate_and_convert_into;
use crate::widgets::{IcedRenderer, Message as WidgetMessage, RenderContext, Widget};

/// Intern a font family name so it gets a `&'static str` without leaking on
/// every config reload. Previously-interned names are reused.
fn intern_font_family(name: &str) -> &'static str {
    static INTERNED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut interned = INTERNED.lock().unwrap();
    if let Some(existing) = interned.iter().find(|s| **s == name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    interned.push(leaked);
    leaked
}

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
    /// Reusable pixmap buffer (`logical_width` × `fb_height`)
    pixmap: Pixmap,
    /// Reusable clip mask (`logical_width` × `fb_height`)
    clip_mask: tiny_skia::Mask,
    /// Reusable rotation output buffer (`fb_height` × `logical_width` × 4 bytes)
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
            Family::Name(intern_font_family(font_family))
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

    pub const fn font(&self) -> Font {
        self.font
    }

    pub const fn font_size(&self) -> f32 {
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
                dn.x_transformed(u32::from(width)) as f32,
                dn.y_transformed(u32::from(height)) as f32,
            );
            touch_positions.insert(dn.seat_slot(), pos);
            Some((
                iced_core::Event::Touch(iced_core::touch::Event::FingerPressed {
                    id: iced_core::touch::Finger(u64::from(dn.seat_slot())),
                    position: pos,
                }),
                mouse::Cursor::Available(pos),
            ))
        }
        input::event::touch::TouchEvent::Motion(mv) => {
            let pos = iced_core::Point::new(
                mv.x_transformed(u32::from(width)) as f32,
                mv.y_transformed(u32::from(height)) as f32,
            );
            touch_positions.insert(mv.seat_slot(), pos);
            Some((
                iced_core::Event::Touch(iced_core::touch::Event::FingerMoved {
                    id: iced_core::touch::Finger(u64::from(mv.seat_slot())),
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
                    id: iced_core::touch::Finger(u64::from(up.seat_slot())),
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

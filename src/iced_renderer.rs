use iced_core::alignment;
use iced_core::clipboard;
use iced_core::layout::{Layout, Limits};
use iced_core::mouse;
use iced_core::renderer;
use iced_core::widget::Tree;
use iced_core::Renderer as _;
use iced_core::font::{Family, Stretch, Style, Weight};
use iced_core::{
    Background, Color, Element, Font, Length, Pixels, Rectangle, Shell, Size, Theme,
};
use iced_graphics::Viewport;
use iced_widget::{container, mouse_area, row, svg, text, Stack};
use tiny_skia::Pixmap;

use crate::battery_icon_widget::BatteryIconWidget;
use crate::memory_graph_widget::MemoryGraphWidget;

type IcedRenderer = iced_tiny_skia::Renderer;

#[derive(Debug, Clone)]
pub enum Message {
    ButtonDown(usize),
    ButtonUp(usize),
    WorkspaceDown(u64),
    WorkspaceUp(u64),
    VolumeDownPress,
    VolumeDownRelease,
    VolumeUpPress,
    VolumeUpRelease,
}

#[derive(Debug, Clone)]
pub enum ButtonAction {
    LayerButton(usize),
    Workspace(u64),
    Volume { down_icon: Option<String>, up_icon: Option<String> },
    None,
}

#[derive(Debug, Clone)]
pub struct BatteryInfo {
    pub capacity: u32,
    pub charging: bool,
    pub blink_on: bool,
    pub time_estimate: Option<String>,
    pub show_time: bool,
}

pub struct ButtonDef {
    pub label: String,
    pub active: bool,
    /// Fraction of available width (0.0–1.0) this button should occupy.
    pub width_fraction: f64,
    /// Optional custom background color as (r, g, b) in 0.0–1.0.
    pub color: Option<(f64, f64, f64)>,
    pub action: ButtonAction,
    /// Memory graph data — when Some, renders a bar graph instead of text.
    pub graph_data: Option<Vec<u32>>,
    /// Maximum columns for the memory graph.
    pub graph_max_columns: Option<usize>,
    /// Battery info — when Some, renders a battery icon with fill level.
    pub battery: Option<BatteryInfo>,
    /// SVG icon file path — when Some, renders the SVG instead of text.
    pub icon: Option<String>,
}

pub struct TouchbarRenderer {
    renderer: IcedRenderer,
    /// Long axis (≈2170)
    logical_width: u32,
    /// Visible short axis from DRM mode (≈60) — used for widget layout
    visible_height: u32,
    /// Framebuffer short axis (64) — used for pixmap/rotation buffer size
    fb_height: u32,
    /// Persistent widget tree — preserves mouse_area hover state across events
    tree: Option<Tree>,
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
            tree: None,
            font,
            font_size,
        }
    }

    /// Sync the persistent tree with the current widget structure.
    /// Preserves compatible state (e.g. mouse_area hover tracking).
    fn sync_tree(&mut self, element: &Element<'_, Message, Theme, IcedRenderer>) {
        match &mut self.tree {
            Some(tree) => element.as_widget().diff(tree),
            slot @ None => *slot = Some(Tree::new(element.as_widget())),
        }
    }

    /// Build the widget tree, render it to a pixmap, and return the
    /// rotated XRGB8888 buffer ready for the DRM framebuffer.
    pub fn render_to_buffer(&mut self, buttons: &[ButtonDef]) -> Vec<u8> {
        self.renderer.clear();

        let element = build_button_row(buttons, self.font, self.font_size);
        self.sync_tree(&element);
        // Take tree out of self to avoid split-borrow issues with self.renderer
        let mut tree = self.tree.take().unwrap();

        let w = self.logical_width;
        let vis_h = self.visible_height;
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
        let mut pixmap = Pixmap::new(w, fb_h).expect("Failed to create pixmap");
        let mut clip_mask = tiny_skia::Mask::new(w, fb_h).expect("Failed to create clip mask");
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

        self.tree = Some(tree);

        // Rotate 90 CW: landscape pixmap (w, fb_h) -> portrait buffer (fb_h, w)
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

    /// Process a touch event through the iced widget tree.
    /// Returns messages produced by widget interactions (ButtonDown/ButtonUp).
    pub fn process_touch(
        &mut self,
        iced_event: iced_core::Event,
        cursor: mouse::Cursor,
        buttons: &[ButtonDef],
    ) -> Vec<Message> {
        let mut element = build_button_row(buttons, self.font, self.font_size);
        self.sync_tree(&element);
        let mut tree = self.tree.take().unwrap();

        let w = self.logical_width;
        let vis_h = self.visible_height;

        let limits = Limits::new(
            Size::ZERO,
            Size::new(w as f32, vis_h as f32),
        );
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

        self.tree = Some(tree);
        messages
    }
}

fn build_button_row(buttons: &[ButtonDef], font: Font, font_size: f32) -> Element<'_, Message, Theme, IcedRenderer> {
    let spacing = 4;
    let padding = 2;

    let children: Vec<Element<'_, Message, Theme, IcedRenderer>> = buttons
        .iter()
        .map(|btn| {
            let bg = match btn.color {
                Some((r, g, b)) => {
                    let scale = if btn.active { 1.0 } else { 0.5 };
                    Color::from_rgb(r as f32 * scale, g as f32 * scale, b as f32 * scale)
                }
                None => {
                    let v = if btn.active { 0.4 } else { 0.2 };
                    Color::from_rgb(v, v, v)
                }
            };
            let portion = (btn.width_fraction * 1000.0).round() as u16;

            // Volume: single button with left/right touch zones for vol down/up
            if let ButtonAction::Volume { ref down_icon, ref up_icon } = btn.action {
                let make_icon = |icon: &Option<String>| -> Element<'_, Message, Theme, IcedRenderer> {
                    if let Some(path) = icon {
                        let handle = svg::Handle::from_path(path);
                        svg::Svg::new(handle)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .content_fit(iced_core::ContentFit::Contain)
                            .into()
                    } else {
                        text("").width(Length::Fill).height(Length::Fill).into()
                    }
                };

                let left: Element<'_, Message, Theme, IcedRenderer> = container(
                    mouse_area(make_icon(down_icon))
                        .on_press(Message::VolumeDownPress)
                        .on_release(Message::VolumeDownRelease)
                        .on_exit(Message::VolumeDownRelease),
                )
                .width(Length::FillPortion(1))
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();

                let center: Element<'_, Message, Theme, IcedRenderer> = container(
                    text(btn.label.to_string())
                        .font(font)
                        .size(font_size)
                        .color(Color::WHITE)
                        .align_x(alignment::Horizontal::Center)
                        .align_y(alignment::Vertical::Center)
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .width(Length::FillPortion(2))
                .height(Length::Fill)
                .into();

                let right: Element<'_, Message, Theme, IcedRenderer> = container(
                    mouse_area(make_icon(up_icon))
                        .on_press(Message::VolumeUpPress)
                        .on_release(Message::VolumeUpRelease)
                        .on_exit(Message::VolumeUpRelease),
                )
                .width(Length::FillPortion(1))
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();

                let vol_row = row(vec![left, center, right])
                    .width(Length::Fill)
                    .height(Length::Fill);

                return container(
                    container(vol_row)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .padding(padding)
                        .style(move |_theme: &Theme| container::Style {
                            background: Some(Background::Color(bg)),
                            border: iced_core::Border {
                                radius: 8.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                )
                .width(Length::FillPortion(portion))
                .height(Length::Fill)
                .into();
            }

            let inner: Element<'_, Message, Theme, IcedRenderer> =
                if let Some(ref info) = btn.battery {
                    if info.show_time {
                        let time_text = info
                            .time_estimate
                            .as_deref()
                            .unwrap_or("N/A");
                        let text_color = if info.charging {
                            Color::from_rgb(0.3, 0.9, 0.3)
                        } else {
                            Color::WHITE
                        };
                        container(
                            text(time_text.to_string())
                                .font(font)
                                .size(font_size * 0.75)
                                .color(text_color)
                                .align_x(alignment::Horizontal::Center)
                                .align_y(alignment::Vertical::Center)
                                .width(Length::Fill)
                                .height(Length::Fill),
                        )
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .padding(padding)
                        .style(move |_theme: &Theme| container::Style {
                            background: Some(Background::Color(bg)),
                            border: iced_core::Border {
                                radius: 8.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .into()
                    } else {
                        let icon = BatteryIconWidget::new(info.capacity, info.charging, info.blink_on);
                        let label = text(btn.label.to_string())
                            .font(font)
                            .size(font_size * 0.75)
                            .color(Color::WHITE)
                            .align_x(alignment::Horizontal::Center)
                            .align_y(alignment::Vertical::Center)
                            .width(Length::Fill)
                            .height(Length::Fill);
                        let stacked: Element<'_, Message, Theme, IcedRenderer> = Stack::new()
                            .push(icon)
                            .push(label)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .into();
                        container(stacked)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .padding(padding)
                            .style(move |_theme: &Theme| container::Style {
                                background: Some(Background::Color(bg)),
                                border: iced_core::Border {
                                    radius: 8.0.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            })
                            .into()
                    }
                } else if let (Some(data), Some(max_cols)) = (&btn.graph_data, btn.graph_max_columns) {
                    container(MemoryGraphWidget::new(data.clone(), max_cols))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .padding(padding)
                        .style(move |_theme: &Theme| container::Style {
                            background: Some(Background::Color(bg)),
                            border: iced_core::Border {
                                radius: 8.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .into()
                } else if let Some(ref icon_path) = btn.icon {
                    let handle = svg::Handle::from_path(icon_path);
                    container(
                        svg::Svg::new(handle)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .content_fit(iced_core::ContentFit::Contain),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(padding)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(move |_theme: &Theme| container::Style {
                        background: Some(Background::Color(bg)),
                        border: iced_core::Border {
                            radius: 8.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .into()
                } else {
                    container(
                        text(btn.label.to_string())
                            .font(font)
                            .size(font_size)
                            .color(Color::WHITE)
                            .align_x(alignment::Horizontal::Center)
                            .align_y(alignment::Vertical::Center)
                            .width(Length::Fill)
                            .height(Length::Fill),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(padding)
                    .style(move |_theme: &Theme| container::Style {
                        background: Some(Background::Color(bg)),
                        border: iced_core::Border {
                            radius: 8.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .into()
                };

            let wrapped: Element<'_, Message, Theme, IcedRenderer> = match &btn.action {
                ButtonAction::LayerButton(idx) => {
                    let idx = *idx;
                    mouse_area(inner)
                        .on_press(Message::ButtonDown(idx))
                        .on_release(Message::ButtonUp(idx))
                        .on_exit(Message::ButtonUp(idx))
                        .into()
                }
                ButtonAction::Workspace(id) => {
                    let id = *id;
                    mouse_area(inner)
                        .on_press(Message::WorkspaceDown(id))
                        .on_release(Message::WorkspaceUp(id))
                        .on_exit(Message::WorkspaceUp(id))
                        .into()
                }
                _ => inner.into(),
            };

            container(wrapped)
                .width(Length::FillPortion(portion))
                .height(Length::Fill)
                .into()
        })
        .collect();

    container(
        row(children)
            .spacing(spacing)
            .height(Length::Fill)
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(2)
    .into()
}

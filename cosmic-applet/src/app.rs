use std::time::Duration;

use cosmic::app::{Core, Task};
use cosmic::iced::platform_specific::shell::wayland::commands::layer_surface::{
    destroy_layer_surface, get_layer_surface, set_margin, Anchor, KeyboardInteractivity, Layer,
};
use cosmic::iced::platform_specific::runtime::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::widget::container::Style as ContainerStyle;
use cosmic::iced::{
    window, Alignment, Background, Border, Color, Length, Limits, Point, Subscription,
};
use cosmic::widget::{container, mouse_area, text, Column, Row, Space};
use cosmic::{Application, Element};

use crate::config::WidgetConfig;
use crate::diagnose;
use crate::models::AppUsageData;
use crate::poller::{self, PollError};
use crate::strings::STRINGS;

const POLL_INTERVAL: Duration = Duration::from_secs(60);
pub const APP_ID: &str = "com.codezeno.CosmicAppletClaudeUsage";

/// Movement (in px) below which a press+release counts as a click, not a drag.
const DRAG_THRESHOLD: f32 = 3.0;

// Segmented-bar layout, mirroring the Windows widget (logical pixels; COSMIC
// applies HiDPI scaling itself, so these are the 96-DPI base sizes).
const SEG_W: f32 = 10.0;
const SEG_H: f32 = 13.0;
const SEG_GAP: f32 = 2.0;
const SEG_RADIUS: f32 = 2.0;
const LABEL_WIDTH: f32 = 22.0;
const VALUE_WIDTH: f32 = 58.0;
const BAR_TEXT_GAP: f32 = 4.0;
const ROW_SPACING: f32 = 10.0;
const ROW_GAP: f32 = 6.0;
const DIVIDER_H: f32 = 26.0;
const DIVIDER_GAP: f32 = 8.0;
const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 5.0;

const WIDGET_HEIGHT: u32 = 58;

/// Number of models currently displayed (at least one).
fn active_model_count(show_claude_code: bool, show_codex: bool) -> u32 {
    (show_claude_code as u32 + show_codex as u32).max(1)
}

/// Segments per bar: 10 for a single model, 5 when two share the row — matching
/// the Windows widget so a full bar is the same visual width regardless.
fn segment_count(models: u32) -> i32 {
    if models >= 2 {
        5
    } else {
        10
    }
}

/// Compute the surface size to fit the segmented-bar layout for the active models.
fn surface_size(show_claude_code: bool, show_codex: bool) -> (u32, u32) {
    let models = active_model_count(show_claude_code, show_codex);
    let segs = segment_count(models) as f32;
    let bar_w = segs * SEG_W + (segs - 1.0) * SEG_GAP;
    let cell_w = bar_w + BAR_TEXT_GAP + VALUE_WIDTH;
    // Row is [label][sp][cell]…[sp][cell]; ROW_SPACING sits before every cell.
    let grid_w = LABEL_WIDTH + models as f32 * (ROW_SPACING + cell_w);
    let width = 2.0 * PAD_X + 3.0 /* divider */ + DIVIDER_GAP + grid_w;
    (width.ceil() as u32, WIDGET_HEIGHT)
}

#[derive(Debug, Clone)]
pub enum Message {
    CreateSurface,
    Tick,
    PollFinished(Result<AppUsageData, PollError>),
    DragStart,
    DragMove(Point),
    DragEnd,
    Quit,
}

struct DragState {
    grab: Point,
    moved: bool,
}

pub struct ClaudeUsageApplet {
    core: Core,
    layer_id: window::Id,
    data: Option<AppUsageData>,
    last_error: Option<PollError>,
    show_claude_code: bool,
    show_codex: bool,
    config: WidgetConfig,
    hover_pos: Point,
    drag: Option<DragState>,
}

impl Application for ClaudeUsageApplet {
    type Message = Message;
    type Executor = cosmic::executor::multi::Executor;
    type Flags = ();
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Message>) {
        let _ = diagnose::init();
        let config = WidgetConfig::load();
        let layer_id = window::Id::unique();

        let app = Self {
            core,
            layer_id,
            data: None,
            last_error: None,
            show_claude_code: true,
            show_codex: true,
            config,
            hover_pos: Point::ORIGIN,
            drag: None,
        };

        // Defer layer-surface creation until after the windowing system has
        // booted. Requesting it directly from `init` races the compositor setup
        // (cosmic ties windowing init to the placeholder main-window id) and the
        // surface never maps.
        let defer_surface = Task::perform(
            async { tokio::time::sleep(Duration::from_millis(250)).await },
            |_| cosmic::action::app(Message::CreateSurface),
        );

        (app, Task::batch([defer_surface, spawn_poll(true, true)]))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::CreateSurface => {
                diagnose::log("creating layer surface");
                let (width, height) = surface_size(self.show_claude_code, self.show_codex);
                return get_layer_surface(SctkLayerSurfaceSettings {
                    id: self.layer_id,
                    layer: Layer::Overlay,
                    keyboard_interactivity: KeyboardInteractivity::None,
                    anchor: Anchor::TOP | Anchor::LEFT,
                    namespace: "claude-usage-widget".into(),
                    margin:
                        cosmic::iced::platform_specific::runtime::wayland::layer_surface::IcedMargin {
                            top: self.config.y,
                            left: self.config.x,
                            right: 0,
                            bottom: 0,
                        },
                    size: Some((Some(width), Some(height))),
                    // Pin the surface to an exact size. A min of 1px lets the
                    // compositor collapse the surface toward nothing (the
                    // "window is too small" bug); min == max forces the size.
                    size_limits: Limits::NONE
                        .min_width(width as f32)
                        .min_height(height as f32)
                        .max_width(width as f32)
                        .max_height(height as f32),
                    ..Default::default()
                });
            }
            Message::Tick => return spawn_poll(self.show_claude_code, self.show_codex),
            Message::PollFinished(result) => match result {
                Ok(data) => {
                    diagnose::log(format!(
                        "poll ok: claude={} codex={}",
                        data.claude_code
                            .as_ref()
                            .map(|d| format!("{:.0}%", d.session.percentage))
                            .unwrap_or_else(|| "none".into()),
                        data.codex
                            .as_ref()
                            .map(|d| format!("{:.0}%", d.session.percentage))
                            .unwrap_or_else(|| "none".into()),
                    ));
                    self.data = Some(data);
                    self.last_error = None;
                }
                Err(error) => {
                    diagnose::log(format!("poll failed: {error:?}"));
                    self.last_error = Some(error);
                }
            },
            Message::DragStart => {
                self.drag = Some(DragState {
                    grab: self.hover_pos,
                    moved: false,
                });
            }
            Message::DragMove(pos) => {
                if let Some(drag) = self.drag.as_mut() {
                    // Servo-follow: move the surface so the grab point stays under
                    // the cursor. Correction is the offset of the cursor from where
                    // it was grabbed; margin is nudged by that each motion event.
                    let dx = pos.x - drag.grab.x;
                    let dy = pos.y - drag.grab.y;
                    if dx.abs() > DRAG_THRESHOLD || dy.abs() > DRAG_THRESHOLD {
                        drag.moved = true;
                    }
                    if drag.moved {
                        self.config.x = (self.config.x + dx.round() as i32).max(0);
                        self.config.y = (self.config.y + dy.round() as i32).max(0);
                        return set_margin(self.layer_id, self.config.y, 0, 0, self.config.x);
                    }
                } else {
                    self.hover_pos = pos;
                }
            }
            Message::DragEnd => {
                if let Some(drag) = self.drag.take() {
                    if drag.moved {
                        self.config.save();
                    } else {
                        // A click without movement = manual refresh.
                        return spawn_poll(self.show_claude_code, self.show_codex);
                    }
                }
            }
            Message::Quit => {
                return Task::batch([
                    destroy_layer_surface(self.layer_id),
                    cosmic::iced::exit(),
                ]);
            }
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        cosmic::iced::time::every(POLL_INTERVAL).map(|_| Message::Tick)
    }

    fn view(&self) -> Element<'_, Message> {
        // No main window; all rendering happens in view_window for the layer surface.
        text("").into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        let is_dark = cosmic::theme::active().cosmic().is_dark;

        let widget = container(self.widget_row(is_dark))
            .padding([PAD_Y, PAD_X])
            .class(solid(bg_color(is_dark), 6.0));

        // The whole surface is draggable. Left press+move = reposition,
        // left click (no move) = refresh, right click = quit.
        mouse_area(widget)
            .on_press(Message::DragStart)
            .on_release(Message::DragEnd)
            .on_right_press(Message::Quit)
            .on_move(Message::DragMove)
            .into()
    }
}

/// One model's rendered data for a row (session or weekly).
struct ModelRender {
    accent: Color,
    value_color: Color,
    percent: f64,
    value_text: String,
}

impl ClaudeUsageApplet {
    fn widget_row(&self, is_dark: bool) -> Element<'_, Message> {
        // Left two-tone divider — the visual drag handle, matching Windows.
        let (div_left, div_right) = divider_colors(is_dark);
        let divider = Row::new()
            .push(
                container(Space::new())
                    .width(Length::Fixed(2.0))
                    .height(Length::Fixed(DIVIDER_H))
                    .class(solid(div_left, 0.0)),
            )
            .push(
                container(Space::new())
                    .width(Length::Fixed(1.0))
                    .height(Length::Fixed(DIVIDER_H))
                    .class(solid(div_right, 0.0)),
            );

        let content: Element<'_, Message> = match &self.data {
            Some(data) => {
                let mut models: Vec<(Color, Color, &crate::models::UsageData)> = Vec::new();
                if self.show_claude_code {
                    if let Some(usage) = data.claude_code.as_ref() {
                        models.push((claude_accent(), claude_value_color(is_dark), usage));
                    }
                }
                if self.show_codex {
                    if let Some(usage) = data.codex.as_ref() {
                        models.push((codex_accent(is_dark), codex_value_color(is_dark), usage));
                    }
                }

                if models.is_empty() {
                    text("--").size(12).class(label_color(is_dark)).into()
                } else {
                    let count = models.len() as u32;
                    let segs = segment_count(count);
                    let use_model_colors = count > 1;
                    let track = track_color(is_dark);

                    let session_cells: Vec<ModelRender> = models
                        .iter()
                        .map(|(accent, value_color, usage)| ModelRender {
                            accent: *accent,
                            value_color: if use_model_colors {
                                *value_color
                            } else {
                                label_color(is_dark)
                            },
                            percent: usage.session.percentage,
                            value_text: poller::format_line(&usage.session, STRINGS),
                        })
                        .collect();
                    let weekly_cells: Vec<ModelRender> = models
                        .iter()
                        .map(|(accent, value_color, usage)| ModelRender {
                            accent: *accent,
                            value_color: if use_model_colors {
                                *value_color
                            } else {
                                label_color(is_dark)
                            },
                            percent: usage.weekly.percentage,
                            value_text: poller::format_line(&usage.weekly, STRINGS),
                        })
                        .collect();

                    Column::new()
                        .spacing(ROW_GAP)
                        .push(usage_row(
                            STRINGS.session_window,
                            session_cells,
                            segs,
                            track,
                            is_dark,
                        ))
                        .push(usage_row(
                            STRINGS.weekly_window,
                            weekly_cells,
                            segs,
                            track,
                            is_dark,
                        ))
                        .into()
                }
            }
            None => {
                let message = match self.last_error {
                    Some(PollError::NoCredentials) => STRINGS.no_credentials,
                    Some(PollError::AuthRequired) | Some(PollError::TokenExpired) => {
                        STRINGS.token_expired
                    }
                    _ => "…",
                };
                text(message).size(12).class(label_color(is_dark)).into()
            }
        };

        Row::new()
            .spacing(DIVIDER_GAP)
            .align_y(Alignment::Center)
            .push(divider)
            .push(content)
            .into()
    }
}

/// One row (session or weekly): a label followed by each model's bar + value.
fn usage_row(
    label: &str,
    cells: Vec<ModelRender>,
    segs: i32,
    track: Color,
    is_dark: bool,
) -> Element<'static, Message> {
    let mut row = Row::new()
        .spacing(ROW_SPACING)
        .align_y(Alignment::Center)
        .push(
            text(label.to_string())
                .size(11)
                .width(Length::Fixed(LABEL_WIDTH))
                .class(label_color(is_dark)),
        );

    for cell in cells {
        row = row.push(
            Row::new()
                .spacing(BAR_TEXT_GAP)
                .align_y(Alignment::Center)
                .push(segmented_bar(cell.percent, segs, cell.accent, track))
                .push(
                    text(cell.value_text)
                        .size(11)
                        .width(Length::Fixed(VALUE_WIDTH))
                        .class(cell.value_color),
                ),
        );
    }

    row.into()
}

/// A horizontal strip of rounded segments, filled to `percent`.
fn segmented_bar(
    percent: f64,
    segs: i32,
    accent: Color,
    track: Color,
) -> Element<'static, Message> {
    let mut row = Row::new().spacing(SEG_GAP).align_y(Alignment::Center);
    for i in 0..segs {
        let color = if seg_filled(percent, i, segs) {
            accent
        } else {
            track
        };
        row = row.push(
            container(Space::new())
                .width(Length::Fixed(SEG_W))
                .height(Length::Fixed(SEG_H))
                .class(solid(color, SEG_RADIUS)),
        );
    }
    row.into()
}

/// Whether segment `i` (of `count`) reads as filled at the given percentage.
/// A boundary segment fills once it is at least half covered — a faithful,
/// robust approximation of the Windows partial-fill at these tiny sizes.
fn seg_filled(percent: f64, i: i32, count: i32) -> bool {
    let seg = 100.0 / count as f64;
    let start = i as f64 * seg;
    let end = start + seg;
    let p = percent.clamp(0.0, 100.0);
    if p >= end {
        true
    } else if p <= start {
        false
    } else {
        (p - start) / seg >= 0.5
    }
}

/// A solid-colour container style with an optional corner radius.
fn solid(color: Color, radius: f32) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |_| ContainerStyle {
        background: Some(Background::Color(color)),
        border: Border {
            radius: radius.into(),
            ..Default::default()
        },
        ..Default::default()
    })
}

fn bg_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_rgb8(0x1C, 0x1C, 0x1C)
    } else {
        Color::from_rgb8(0xF3, 0xF3, 0xF3)
    }
}

fn track_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_rgb8(0x44, 0x44, 0x44)
    } else {
        Color::from_rgb8(0xAA, 0xAA, 0xAA)
    }
}

fn label_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_rgb8(0x88, 0x88, 0x88)
    } else {
        Color::from_rgb8(0x40, 0x40, 0x40)
    }
}

fn divider_colors(is_dark: bool) -> (Color, Color) {
    if is_dark {
        (
            Color::from_rgb8(0x50, 0x50, 0x50),
            Color::from_rgb8(0x28, 0x28, 0x28),
        )
    } else {
        (
            Color::from_rgb8(0xA0, 0xA0, 0xA0),
            Color::from_rgb8(0xE6, 0xE6, 0xE6),
        )
    }
}

fn claude_accent() -> Color {
    Color::from_rgb8(0xD9, 0x77, 0x57)
}

fn codex_accent(is_dark: bool) -> Color {
    if is_dark {
        Color::from_rgb8(0xF5, 0xF5, 0xF5)
    } else {
        Color::from_rgb8(0x1F, 0x1F, 0x1F)
    }
}

fn claude_value_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_rgb8(0xF0, 0x9A, 0x7A)
    } else {
        Color::from_rgb8(0xA9, 0x4F, 0x32)
    }
}

fn codex_value_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_rgb8(0xF5, 0xF5, 0xF5)
    } else {
        Color::from_rgb8(0x1F, 0x1F, 0x1F)
    }
}

fn spawn_poll(show_claude_code: bool, show_codex: bool) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || poller::poll(show_claude_code, show_codex))
                .await
                .unwrap_or(Err(PollError::RequestFailed))
        },
        |result| cosmic::action::app(Message::PollFinished(result)),
    )
}

use std::time::Duration;

use cosmic::app::{Core, Task};
use cosmic::iced::platform_specific::shell::wayland::commands::layer_surface::{
    destroy_layer_surface, get_layer_surface, set_margin, Anchor, KeyboardInteractivity, Layer,
};
use cosmic::iced::platform_specific::runtime::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::{window, Limits, Point, Subscription};
use cosmic::widget::{container, mouse_area, text, Column, Row};
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

const WIDGET_WIDTH: u32 = 300;
const WIDGET_HEIGHT: u32 = 40;

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
                    size: Some((Some(WIDGET_WIDTH), Some(WIDGET_HEIGHT))),
                    size_limits: Limits::NONE
                        .min_width(1.0)
                        .min_height(1.0)
                        .max_width(WIDGET_WIDTH as f32)
                        .max_height(WIDGET_HEIGHT as f32),
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
        let widget = container(self.widget_row())
            .padding([4, 8])
            .class(cosmic::theme::Container::Background);

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

impl ClaudeUsageApplet {
    fn widget_row(&self) -> Element<'_, Message> {
        let mut row = Row::new().spacing(10).align_y(cosmic::iced::Alignment::Center);

        match &self.data {
            Some(data) => {
                if self.show_claude_code {
                    row = row.push(model_cell("Claude", data.claude_code.as_ref()));
                }
                if self.show_codex {
                    row = row.push(model_cell("Codex", data.codex.as_ref()));
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
                row = row.push(text(message).size(12));
            }
        }

        row.into()
    }
}

fn model_cell(label: &str, data: Option<&crate::models::UsageData>) -> Element<'static, Message> {
    match data {
        Some(data) => Column::new()
            .spacing(0)
            .push(text(label.to_string()).size(10))
            .push(
                text(format!(
                    "{} {}  {} {}",
                    STRINGS.session_window,
                    poller::format_line(&data.session, STRINGS),
                    STRINGS.weekly_window,
                    poller::format_line(&data.weekly, STRINGS),
                ))
                .size(12),
            )
            .into(),
        None => text(format!("{label} --")).size(12).into(),
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

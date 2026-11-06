use super::AudioBlock;
use crate::core::runtime::runtime;
use crate::ui::media::{
    audio::AudioAssets,
    source::{MediaFetcher, MediaSource, Parsed},
};
use crate::ui::rich::narrative::callout::GlyphFactory;
use crate::ui::theme::{ArtisanTheme, Oklch, Opacity};
use aqua_core::{GpuiBackground, InputEvent, PointerButton, Rect};
use aqua_text::{FontFamily, FontWeight, TextStyle, overflow::OverflowMode};
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{CaptureRequest, EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use editor_assets::AssetId;
use editor_audio::{PlaybackState, Player, Snapshot};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tokio_util::task::AbortOnDropHandle;

static NEXT_WAITER: AtomicU64 = AtomicU64::new(1);
pub(crate) type ClickFactory = Rc<dyn Fn(Element, Rc<dyn Fn()>) -> Element>;
struct Row {
    waiter: u64,
    line: Rc<Cell<Option<Rect>>>,
    dragging: bool,
    hovered: bool,
    timer: Option<AbortOnDropHandle<()>>,
}
struct State {
    rows: HashMap<String, Row>,
    wake: aqua::mailbox::Mailbox<()>,
}
#[derive(Clone)]
pub(crate) struct AudioState(Rc<RefCell<State>>);
impl Default for AudioState {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(State {
            rows: HashMap::new(),
            wake: aqua::mailbox::Mailbox::new(NonZeroUsize::MIN),
        })))
    }
}
impl AudioState {
    pub(crate) fn bind(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        ctx.bind_mailbox(window, &self.0.borrow().wake);
    }
    pub(crate) fn drain(&self) -> bool {
        !self.0.borrow_mut().wake.drain().is_empty()
    }
    pub(crate) fn release(&self, id: &str) {
        let prefix = format!("{id}-track-");
        self.0
            .borrow_mut()
            .rows
            .retain(|key, _| !key.starts_with(&prefix));
    }
    fn retain_rows(&self, id: &str, count: usize) {
        let prefix = format!("{id}-track-");
        self.0.borrow_mut().rows.retain(|key, _| {
            key.strip_prefix(&prefix)
                .and_then(|index| index.parse::<usize>().ok())
                .is_none_or(|index| index < count)
        });
    }
    fn row(&self, id: &str) -> (u64, Rc<Cell<Option<Rect>>>, bool) {
        let mut state = self.0.borrow_mut();
        let row = state.rows.entry(id.to_owned()).or_insert_with(|| Row {
            waiter: NEXT_WAITER.fetch_add(1, Ordering::Relaxed),
            line: Rc::new(Cell::new(None)),
            dragging: false,
            hovered: false,
            timer: None,
        });
        (row.waiter, row.line.clone(), row.hovered)
    }
    fn tick(&self, id: &str, live: bool) {
        let mut state = self.0.borrow_mut();
        let sender = state.wake.sender();
        let Some(row) = state.rows.get_mut(id) else {
            return;
        };
        if live == row.timer.is_some() {
            return;
        }
        row.timer = live.then(|| {
            AbortOnDropHandle::new(runtime().spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    match sender.send(()) {
                        Ok(()) | Err(aqua::mailbox::SendError::Full(())) => {}
                        Err(aqua::mailbox::SendError::Closed(())) => break,
                    }
                }
            }))
        });
    }
}
#[derive(Clone, Debug, PartialEq)]
enum Shown {
    Idle,
    Loading,
    Playing {
        position: Duration,
        duration: Duration,
    },
    Paused {
        position: Duration,
        duration: Duration,
    },
    Ended {
        duration: Duration,
    },
    Failed(String),
}
impl Shown {
    fn of(snapshot: &Snapshot, path: &Path) -> Self {
        if snapshot.track.as_deref() != Some(path) {
            return Self::Idle;
        }
        let duration = snapshot.duration.unwrap_or_default();
        let position = snapshot.position.min(duration);
        match &snapshot.state {
            PlaybackState::Idle => Self::Idle,
            PlaybackState::Loading => Self::Loading,
            PlaybackState::Playing => Self::Playing { position, duration },
            PlaybackState::Paused => Self::Paused { position, duration },
            PlaybackState::Ended => Self::Ended { duration },
            PlaybackState::Failed(message) => Self::Failed(message.clone()),
        }
    }
    fn duration(&self) -> Option<Duration> {
        match self {
            Self::Playing { duration, .. }
            | Self::Paused { duration, .. }
            | Self::Ended { duration } => Some(*duration),
            _ => None,
        }
    }
    fn progress(&self) -> f32 {
        match self {
            Self::Playing { position, duration } | Self::Paused { position, duration }
                if !duration.is_zero() =>
            {
                (position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
            }
            Self::Ended { .. } => 1.0,
            _ => 0.0,
        }
    }
}
fn clock(time: Duration) -> String {
    let seconds = time.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}
fn text(
    theme: &ArtisanTheme,
    value: &str,
    size: f32,
    line: f32,
    weight: FontWeight,
    color: Oklch,
    mono: bool,
) -> Element {
    let font = if mono {
        theme.typography.mono.family
    } else {
        theme.typography.sans.family
    };
    let style = TextStyle {
        families: vec![FontFamily::Named(font.to_owned())],
        size_px: size,
        line_height_px: line,
        weight,
        ..TextStyle::default()
    };
    ui::Text::new(value.to_owned())
        .text_style(style)
        .gpui_color(color.to_reference_paint())
        .wrap(false)
        .overflow(OverflowMode::EndEllipsis)
        .build()
}
fn row() -> ui::Box {
    ui::Box::new().layout("row").style(
        Style::new()
            .width(Length::Pct(100.0))
            .min_width(Length::Px(0.0))
            .min_height(Length::Px(32.0))
            .align(aqua_ui::AlignItems::CENTER)
            .gap(10.0),
    )
}
fn track_path(source: &MediaSource, fetcher: Option<&MediaFetcher>, id: &str) -> Option<PathBuf> {
    match &source.parsed {
        Parsed::Data(payload) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            payload.bytes.as_ref().hash(&mut hasher);
            Some(PathBuf::from(format!("data/{}.audio", hasher.finish())))
        }
        Parsed::File { path, .. } => {
            let fetcher = fetcher?;
            let owner = fetcher.asset_owner(id, false);
            Some(PathBuf::from(format!(
                "forge/{}/{owner}/{path}",
                fetcher.scope()
            )))
        }
        _ => None,
    }
}
fn seek(player: &Player, line: &Rc<Cell<Option<Rect>>>, duration: Option<Duration>, x: f32) {
    if let (Some(bounds), Some(duration)) = (line.get(), duration) {
        let share = ((x - bounds.x) / bounds.w.max(1.0)).clamp(0.0, 1.0);
        player.seek(duration.mul_f32(share));
    }
}

impl AudioBlock {
    pub(crate) fn view(
        &self,
        id: &str,
        theme: &ArtisanTheme,
        fetcher: Option<&MediaFetcher>,
        assets: &AudioAssets,
        player: &Player,
        state: AudioState,
        glyph: GlyphFactory,
        click: ClickFactory,
        invalidate: Rc<dyn Fn()>,
    ) -> Element {
        let theme = *theme;
        state.retain_rows(id, self.tracks.len());
        let mut root = ui::Box::new().layout("column").style(
            Style::new()
                .width(Length::Pct(100.0))
                .min_width(Length::Px(0.0))
                .gap(4.0),
        );
        for (index, track) in self.tracks.iter().enumerate() {
            let row_id = format!("{id}-track-{index}");
            let (waiter, line, hovered) = state.row(&row_id);
            let sender = state.0.borrow().wake.sender();
            let wake: aqua::mailbox::Wake = Arc::new(move || {
                let _ = sender.send(());
            });
            let name = track.shown_name();
            let result = match &track.src.parsed {
                Parsed::Url(_) => Err("Only local audio files play here".to_owned()),
                Parsed::Invalid => Err(format!("Couldn't load {}", track.src.trim())),
                Parsed::File { .. } if fetcher.is_none() => {
                    Err(format!("Couldn't load {}", track.src.trim()))
                }
                _ => match assets.request(fetcher, &track.src, &row_id, waiter, wake) {
                    None => Err("Loading…".to_owned()),
                    Some(None) => Err(format!("Couldn't load {}", track.src.trim())),
                    Some(Some(bytes)) => track_path(&track.src, fetcher, &row_id)
                        .map(|path| (path, bytes))
                        .ok_or_else(|| format!("Couldn't load {}", track.src.trim())),
                },
            };
            let (path, bytes) = match result {
                Ok(playable) => playable,
                Err(message) => {
                    state.tick(&row_id, false);
                    root = root.child(
                        row()
                            .child(glyph(
                                AssetId::TABLER_PLAYER_PLAY_FILLED,
                                14.0,
                                theme.colors.text_muted.to_reference_paint(),
                            ))
                            .child(
                                ui::Box::new()
                                    .style(Style::new().flex_shrink(0.0))
                                    .child(crate::ui::rich::narrative::callout::text(
                                        &theme,
                                        &name,
                                        14.0,
                                        22.0,
                                        FontWeight::NORMAL,
                                        theme.colors.text,
                                    ))
                                    .build(),
                            )
                            .child(text(
                                &theme,
                                &message,
                                13.0,
                                20.0,
                                FontWeight::NORMAL,
                                theme.colors.text_muted,
                                false,
                            ))
                            .build(),
                    );
                    continue;
                }
            };
            let shown = Shown::of(&player.snapshot(), &path);
            state.tick(
                &row_id,
                matches!(shown, Shown::Loading | Shown::Playing { .. }),
            );
            let button_color = if hovered {
                theme.colors.pressed
            } else {
                theme.colors.hover
            };
            let icon = if matches!(shown, Shown::Playing { .. } | Shown::Loading) {
                AssetId::TABLER_PLAYER_PAUSE_FILLED
            } else {
                AssetId::TABLER_PLAYER_PLAY_FILLED
            };
            let hovering = state.clone();
            let hover_id = row_id.clone();
            let hover_invalidate = invalidate.clone();
            let button = ui::Box::new()
                .style(
                    Style::new()
                        .width(Length::Px(28.0))
                        .height(Length::Px(28.0))
                        .flex_shrink(0.0)
                        .align(aqua_ui::AlignItems::CENTER)
                        .justify(aqua_ui::JustifyContent::CENTER)
                        .gpui_background(GpuiBackground::Solid(button_color.to_reference_paint())),
                )
                .radius(14.0)
                .child(glyph(icon, 12.0, theme.colors.text.to_reference_paint()))
                .build()
                .with_key(format!("{row_id}-button"))
                .cursor(aqua_core::CursorIcon::Pointer)
                .on_hover_change(move |over| {
                    if let Some(row) = hovering.0.borrow_mut().rows.get_mut(&hover_id) {
                        row.hovered = over;
                    }
                    hover_invalidate();
                });
            let toggle_player = player.clone();
            let toggle_shown = shown.clone();
            let toggle_path = path.clone();
            let toggle_bytes = bytes.clone();
            let toggle_invalidate = invalidate.clone();
            let button = click(
                button,
                Rc::new(move || {
                    match &toggle_shown {
                        Shown::Playing { .. } | Shown::Loading => toggle_player.pause(),
                        Shown::Paused { .. } | Shown::Ended { .. } => toggle_player.resume(),
                        Shown::Idle | Shown::Failed(_) => {
                            toggle_player.play_bytes(&toggle_path, toggle_bytes.clone())
                        }
                    }
                    toggle_invalidate();
                }),
            );
            let title = ui::Box::new()
                .style(Style::new().flex_shrink(0.0).max_width(Length::Pct(40.0)))
                .clip(true)
                .child(text(
                    &theme,
                    &name,
                    14.0,
                    22.0,
                    FontWeight::MEDIUM,
                    theme.colors.text,
                    false,
                ))
                .build();
            let detail = match &shown {
                Shown::Failed(message) => ui::Box::new()
                    .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                    .clip(true)
                    .child(text(
                        &theme,
                        message,
                        13.0,
                        20.0,
                        FontWeight::NORMAL,
                        theme.colors.banner_error.text,
                        false,
                    ))
                    .build(),
                _ => progress_line(
                    &row_id,
                    &theme,
                    &shown,
                    player.clone(),
                    state.clone(),
                    line,
                    invalidate.clone(),
                ),
            };
            let time = match shown {
                Shown::Playing { position, duration } | Shown::Paused { position, duration } => {
                    Some(format!("{} / {}", clock(position), clock(duration)))
                }
                Shown::Ended { duration } => Some(clock(duration)),
                Shown::Loading => Some("Loading…".to_owned()),
                Shown::Idle | Shown::Failed(_) => None,
            };
            let mut result = row().child(button).child(title).child(detail);
            if let Some(time) = time {
                result = result.child(
                    ui::Box::new()
                        .style(Style::new().flex_shrink(0.0))
                        .child(text(
                            &theme,
                            &time,
                            12.0,
                            20.0,
                            FontWeight::NORMAL,
                            theme.colors.text_muted,
                            true,
                        ))
                        .build(),
                );
            }
            root = root.child(result.build());
        }
        root.build()
    }
}

fn progress_line(
    id: &str,
    theme: &ArtisanTheme,
    shown: &Shown,
    player: Player,
    state: AudioState,
    line: Rc<Cell<Option<Rect>>>,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    let duration = shown.duration();
    let progress = shown.progress();
    let background = theme.colors.text.at(Opacity::Rule).to_reference_paint();
    let foreground = theme.colors.text_muted.to_reference_paint();
    let draw_line = line.clone();
    let track = ui::Canvas::new()
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Px(4.0)),
        )
        .on_paint(move |bounds| {
            draw_line.set(Some(bounds));
            vec![
                aqua_core::DisplayItem::GpuiBackground {
                    bounds,
                    corners: aqua_core::CornerRadii::uniform(2.0),
                    background: GpuiBackground::Solid(background),
                },
                aqua_core::DisplayItem::GpuiBackground {
                    bounds: Rect {
                        x: bounds.x,
                        y: bounds.y,
                        w: bounds.w * progress,
                        h: bounds.h,
                    },
                    corners: aqua_core::CornerRadii::uniform(2.0),
                    background: GpuiBackground::Solid(foreground),
                },
            ]
        })
        .build();
    let mut element = ui::Box::new()
        .style(
            Style::new()
                .flex_grow(1.0)
                .min_width(Length::Px(48.0))
                .height(Length::Px(28.0))
                .align(aqua_ui::AlignItems::CENTER),
        )
        .child(track)
        .build()
        .with_key(format!("{id}-line"));
    if duration.is_some() {
        let pressing = state.clone();
        let press_id = id.to_owned();
        let press_player = player.clone();
        let press_line = line.clone();
        let press_invalidate = invalidate.clone();
        let moving = state.clone();
        let move_id = id.to_owned();
        let move_player = player.clone();
        let move_line = line.clone();
        let move_invalidate = invalidate.clone();
        let releasing = state.clone();
        let release_id = id.to_owned();
        element = element
            .cursor(aqua_core::CursorIcon::Pointer)
            .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::PointerDown(pointer, PointerButton::Primary)) =
                    &event.input
                else {
                    return InputResponse::default();
                };
                seek(&press_player, &press_line, duration, pointer.position.x);
                if let Some(row) = pressing.0.borrow_mut().rows.get_mut(&press_id) {
                    row.dragging = true;
                }
                press_invalidate();
                InputResponse {
                    stop_propagation: true,
                    pointer_capture: CaptureRequest::Capture,
                    ..InputResponse::default()
                }
            })
            .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input else {
                    return InputResponse::default();
                };
                if pointer.buttons & 1 != 0
                    && moving
                        .0
                        .borrow()
                        .rows
                        .get(&move_id)
                        .is_some_and(|row| row.dragging)
                {
                    seek(&move_player, &move_line, duration, pointer.position.x);
                    move_invalidate();
                }
                InputResponse::default()
            })
            .on_input(InputKind::PointerUp, EventPhase::Bubble, move |event| {
                if !matches!(
                    &event.input,
                    RoutedInput::Native(InputEvent::PointerUp(_, PointerButton::Primary))
                ) {
                    return InputResponse::default();
                }
                if let Some(row) = releasing.0.borrow_mut().rows.get_mut(&release_id) {
                    row.dragging = false;
                }
                invalidate();
                InputResponse {
                    pointer_capture: CaptureRequest::Release,
                    ..InputResponse::default()
                }
            });
    }
    element
}

#[cfg(feature = "test-support")]
pub struct AudioFixture;
#[cfg(feature = "test-support")]
impl AudioFixture {
    pub fn shown(snapshot: &Snapshot, path: &Path) -> (String, f32, Option<String>) {
        let shown = Shown::of(snapshot, path);
        let label = match &shown {
            Shown::Idle => "idle",
            Shown::Loading => "loading",
            Shown::Playing { .. } => "playing",
            Shown::Paused { .. } => "paused",
            Shown::Ended { .. } => "ended",
            Shown::Failed(_) => "failed",
        };
        let time = match &shown {
            Shown::Playing { position, duration } | Shown::Paused { position, duration } => {
                Some(format!("{} / {}", clock(*position), clock(*duration)))
            }
            Shown::Ended { duration } => Some(clock(*duration)),
            Shown::Loading => Some("Loading…".to_owned()),
            _ => None,
        };
        (label.to_owned(), shown.progress(), time)
    }
    pub fn data_track(source: &str) -> Option<PathBuf> {
        let source = MediaSource::from(source.to_owned());
        track_path(&source, None, "test")
    }
}

use super::AudioBlock;
use crate::ui::kit::Space;
use crate::ui::kit::control::focus_ring;
use crate::ui::kit::{Button, ButtonSize, ButtonTone, Corner, IconSize, TextRole};

const PLAY: f32 = 28.0;
const SEEK_STEP: Duration = Duration::from_secs(5);
use crate::ui::media::{
    audio::{AudioAssets, AudioLoadError},
    source::{MediaFetcher, MediaLoadError, MediaSource, Parsed},
};
use crate::ui::rich::narrative::callout::GlyphFactory;
use crate::ui::theme::{ArtisanTheme, Oklch, Opacity};
use aqua_core::{
    GpuiBackground, InputEvent, Key, NamedKey, NumericRangeSemantics, PointerButton, Rect, Role,
};
use aqua_text::{FontFamily, overflow::OverflowMode};
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{CaptureRequest, EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use aqua_ui::tree::{AccessibilityRequest, ActivationSource};
use editor_assets::AssetId;
use editor_audio::{PlaybackState, Player, Snapshot};
use editor_core::runtime::runtime;
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
struct Row {
    waiter: u64,
    line: Rc<Cell<Option<Rect>>>,
    dragging: bool,
    hovered: bool,
    transcript: Disclosure,
    timer: Option<AbortOnDropHandle<()>>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Disclosure {
    Closed,
    Open,
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
    fn row(&self, id: &str) -> (u64, Rc<Cell<Option<Rect>>>, bool, Disclosure) {
        let mut state = self.0.borrow_mut();
        let row = state.rows.entry(id.to_owned()).or_insert_with(|| Row {
            waiter: NEXT_WAITER.fetch_add(1, Ordering::Relaxed),
            line: Rc::new(Cell::new(None)),
            dragging: false,
            hovered: false,
            transcript: Disclosure::Closed,
            timer: None,
        });
        (
            row.waiter,
            Rc::clone(&row.line),
            row.hovered,
            row.transcript,
        )
    }
    fn toggle_transcript(&self, id: &str) {
        if let Some(row) = self.0.borrow_mut().rows.get_mut(id) {
            row.transcript = match row.transcript {
                Disclosure::Closed => Disclosure::Open,
                Disclosure::Open => Disclosure::Closed,
            };
        }
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
            Self::Idle | Self::Loading | Self::Failed(_) => None,
        }
    }
    fn position(&self) -> Duration {
        match self {
            Self::Playing { position, .. } | Self::Paused { position, .. } => *position,
            Self::Ended { duration } => *duration,
            Self::Idle | Self::Loading | Self::Failed(_) => Duration::ZERO,
        }
    }
    fn time(&self) -> Option<String> {
        match self {
            Self::Playing { position, duration } | Self::Paused { position, duration } => {
                Some(format!("{} / {}", clock(*position), clock(*duration)))
            }
            Self::Ended { duration } => Some(clock(*duration)),
            Self::Loading => Some("Loading…".to_owned()),
            Self::Idle | Self::Failed(_) => None,
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
            Self::Idle
            | Self::Loading
            | Self::Playing { .. }
            | Self::Paused { .. }
            | Self::Failed(_) => 0.0,
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
fn text(theme: &ArtisanTheme, value: &str, role: TextRole, color: Oklch, mono: bool) -> Element {
    let mut style = role.style(theme);
    if mono {
        style.families = vec![FontFamily::Named(theme.typography.mono.family.to_owned())];
    }
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
            .gap(Space::S8.px()),
    )
}
fn track_path(source: &MediaSource, fetcher: Option<&MediaFetcher>, id: &str) -> Option<PathBuf> {
    match source.parsed() {
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
        Parsed::Url(_) | Parsed::Invalid => None,
    }
}
type Seek = Rc<dyn Fn(Duration)>;
fn seek_at(seek: &Seek, line: &Rc<Cell<Option<Rect>>>, duration: Option<Duration>, x: f32) {
    if let (Some(bounds), Some(duration)) = (line.get(), duration) {
        let share = ((x - bounds.x) / bounds.w.max(1.0)).clamp(0.0, 1.0);
        seek(duration.mul_f32(share));
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum SeekCommand {
    Back,
    Forward,
    Start,
    End,
    To(f64),
}
impl SeekCommand {
    fn of_key(key: Key) -> Option<Self> {
        match key {
            Key::Named(NamedKey::Left | NamedKey::Down) => Some(Self::Back),
            Key::Named(NamedKey::Right | NamedKey::Up) => Some(Self::Forward),
            Key::Named(NamedKey::Home) => Some(Self::Start),
            Key::Named(NamedKey::End) => Some(Self::End),
            Key::Char(_) | Key::Named(_) => None,
        }
    }
    fn of_request(request: AccessibilityRequest) -> Option<Self> {
        match request {
            AccessibilityRequest::SetValue(seconds) => Some(Self::To(seconds)),
            AccessibilityRequest::Increment => Some(Self::Forward),
            AccessibilityRequest::Decrement => Some(Self::Back),
            AccessibilityRequest::ShowContextMenu => None,
        }
    }
    fn target(self, position: Duration, duration: Duration) -> Duration {
        match self {
            Self::Back => position.saturating_sub(SEEK_STEP),
            Self::Forward => (position + SEEK_STEP).min(duration),
            Self::Start => Duration::ZERO,
            Self::End => duration,
            Self::To(seconds) => {
                Duration::try_from_secs_f64(seconds.clamp(0.0, duration.as_secs_f64()))
                    .unwrap_or(Duration::ZERO)
            }
        }
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "the play button reads its row, state, name, hover, glyph and both callbacks"
)]
fn play_button(
    row_id: &str,
    theme: &ArtisanTheme,
    shown: &Shown,
    name: &str,
    hovered: bool,
    glyph: &GlyphFactory,
    hover: Rc<dyn Fn(bool)>,
    toggle: Rc<dyn Fn()>,
) -> Element {
    let button_color = if hovered {
        theme.colors.pressed
    } else {
        theme.colors.hover
    };
    let (icon, action) = if matches!(shown, Shown::Playing { .. } | Shown::Loading) {
        (AssetId::TABLER_PLAYER_PAUSE_FILLED, "Pause")
    } else {
        (AssetId::TABLER_PLAYER_PLAY_FILLED, "Play")
    };
    ui::Button::new()
        .style(
            Style::new()
                .width(Length::Px(PLAY))
                .height(Length::Px(PLAY))
                .flex_shrink(0.0)
                .align(aqua_ui::AlignItems::CENTER)
                .justify(aqua_ui::JustifyContent::CENTER)
                .gpui_background(GpuiBackground::Solid(button_color.to_reference_paint())),
        )
        .radius(Corner::Full.resolve(PLAY))
        .child(glyph(
            icon,
            IconSize::Sm.px(),
            theme.colors.text.to_reference_paint(),
        ))
        .on_activate_event(move |event| {
            let secondary = matches!(
                event.source,
                Some(ActivationSource::Pointer { button, .. }) if button != PointerButton::Primary
            );
            if !secondary {
                toggle();
            }
        })
        .build()
        .with_key(format!("{row_id}-button"))
        .role(Role::Button)
        .aria_label(format!("{action} {name}"))
        .gpui_keyboard_activation()
        .focusable(true)
        .focus_visible_style(focus_ring(theme))
        .cursor(aqua_core::CursorIcon::Pointer)
        .on_hover_change(move |over| hover(over))
}
fn transcript_view(
    row_id: &str,
    theme: &ArtisanTheme,
    transcript: &str,
    disclosure: Disclosure,
    state: AudioState,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    let open = disclosure == Disclosure::Open;
    let toggle_id = row_id.to_owned();
    let toggle = Button::text(
        format!("{row_id}-transcript"),
        *theme,
        ButtonTone::Quiet,
        if open {
            "Hide transcript"
        } else {
            "Show transcript"
        },
    )
    .size(ButtonSize::Sm)
    .on_activate(move |_| {
        state.toggle_transcript(&toggle_id);
        invalidate();
    })
    .build()
    .aria_expanded(open);
    let mut column = ui::Box::new().layout("column").style(
        Style::new()
            .width(Length::Pct(100.0))
            .min_width(Length::Px(0.0))
            .align(aqua_ui::AlignItems::START)
            .gap(Space::S4.px()),
    );
    column = column.child(toggle);
    if open {
        column = column.child(crate::ui::rich::narrative::callout::text(
            theme,
            transcript,
            TextRole::Label,
            theme.colors.text,
        ));
    }
    column.build()
}

impl AudioBlock {
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the audio view takes fetcher, assets, player, state and glyph; one view of every track state"
    )]
    pub(crate) fn view(
        &self,
        id: &str,
        theme: &ArtisanTheme,
        fetcher: Option<&MediaFetcher>,
        assets: &AudioAssets,
        player: &Player,
        state: &AudioState,
        glyph: &GlyphFactory,
        invalidate: &Rc<dyn Fn()>,
    ) -> Element {
        let theme = *theme;
        state.retain_rows(id, self.tracks.len());
        let mut root = ui::Box::new().layout("column").style(
            Style::new()
                .width(Length::Pct(100.0))
                .min_width(Length::Px(0.0))
                .gap(Space::S4.px()),
        );
        for (index, track) in self.tracks.iter().enumerate() {
            let row_id = format!("{id}-track-{index}");
            let (waiter, line, hovered, disclosure) = state.row(&row_id);
            let transcript = track.transcript.as_deref().map(|transcript| {
                transcript_view(
                    &row_id,
                    &theme,
                    transcript,
                    disclosure,
                    state.clone(),
                    Rc::clone(invalidate),
                )
            });
            let wake = crate::ui::media::wake_on(&state.0.borrow().wake);
            let name = track.shown_name();
            let result = match assets.request(fetcher, &track.src, &row_id, waiter, wake) {
                None => Err(Unplayable::Loading),
                Some(Err(AudioLoadError::Remote)) => Err(Unplayable::Failed {
                    problem: editor_core::Problem::new("Only local audio files play here."),
                    retryable: false,
                }),
                Some(Err(error)) => Err(Unplayable::Failed {
                    retryable: matches!(error, AudioLoadError::Load(MediaLoadError::Fetch(_))),
                    problem: editor_core::Problem::caused_by(
                        format!("Couldn't load {name}."),
                        &error,
                    ),
                }),
                Some(Ok(bytes)) => track_path(&track.src, fetcher, &row_id)
                    .map(|path| (path, bytes))
                    .ok_or_else(|| Unplayable::Failed {
                        problem: editor_core::Problem::detailed(
                            format!("Couldn't load {name}."),
                            format!("{} has no local path to play from", track.src.trim()),
                        ),
                        retryable: false,
                    }),
            };
            let (path, bytes) = match result {
                Ok(playable) => playable,
                Err(unplayable) => {
                    state.tick(&row_id, false);
                    let track_row = row()
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
                                    TextRole::Label,
                                    theme.colors.text,
                                ))
                                .build(),
                        );
                    root = match unplayable {
                        Unplayable::Loading => root.child(
                            track_row
                                .child(text(
                                    &theme,
                                    "Loading…",
                                    TextRole::Label,
                                    theme.colors.text_muted,
                                    false,
                                ))
                                .build(),
                        ),
                        Unplayable::Failed { problem, retryable } => {
                            let mut notice = crate::ui::kit::ProblemNotice::new(
                                format!("{row_id}-problem"),
                                theme,
                                &problem,
                            );
                            if retryable {
                                let assets = assets.clone();
                                let fetcher = fetcher.cloned();
                                let source = track.src.clone();
                                let part = row_id.clone();
                                let invalidate = Rc::clone(invalidate);
                                notice = notice.action(
                                    Button::text(
                                        format!("{row_id}-retry"),
                                        theme,
                                        ButtonTone::Neutral,
                                        "Try again",
                                    )
                                    .size(ButtonSize::Sm)
                                    .on_activate(move |_| {
                                        assets.forget(fetcher.as_ref(), &source, &part);
                                        invalidate();
                                    })
                                    .build(),
                                );
                            }
                            root.child(track_row.build()).child(notice.build())
                        }
                    };
                    root = root.children(transcript);
                    continue;
                }
            };
            let shown = Shown::of(&player.snapshot(), &path);
            state.tick(
                &row_id,
                matches!(shown, Shown::Loading | Shown::Playing { .. }),
            );
            let hovering = state.clone();
            let hover_id = row_id.clone();
            let hover_invalidate = Rc::clone(invalidate);
            let toggle_player = player.clone();
            let toggle_shown = shown.clone();
            let toggle_path = path.clone();
            let toggle_bytes = Arc::clone(&bytes);
            let toggle_invalidate = Rc::clone(invalidate);
            let button = play_button(
                &row_id,
                &theme,
                &shown,
                &name,
                hovered,
                glyph,
                Rc::new(move |over| {
                    if let Some(row) = hovering.0.borrow_mut().rows.get_mut(&hover_id) {
                        row.hovered = over;
                    }
                    hover_invalidate();
                }),
                Rc::new(move || {
                    match &toggle_shown {
                        Shown::Playing { .. } | Shown::Loading => toggle_player.pause(),
                        Shown::Paused { .. } | Shown::Ended { .. } => toggle_player.resume(),
                        Shown::Idle | Shown::Failed(_) => {
                            toggle_player.play_bytes(&toggle_path, Arc::clone(&toggle_bytes));
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
                    TextRole::LabelStrong,
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
                        TextRole::Label,
                        theme.colors.banner_error.text,
                        false,
                    ))
                    .build(),
                Shown::Idle
                | Shown::Loading
                | Shown::Playing { .. }
                | Shown::Paused { .. }
                | Shown::Ended { .. } => progress_line(
                    &row_id,
                    &theme,
                    &shown,
                    &name,
                    {
                        let player = player.clone();
                        Rc::new(move |to| player.seek(to))
                    },
                    state.clone(),
                    line,
                    Rc::clone(invalidate),
                ),
            };
            let time = shown.time();
            let mut result = row().child(button).child(title).child(detail);
            if let Some(time) = time {
                result = result.child(
                    ui::Box::new()
                        .style(Style::new().flex_shrink(0.0))
                        .child(text(
                            &theme,
                            &time,
                            TextRole::Caption,
                            theme.colors.text_muted,
                            true,
                        ))
                        .build(),
                );
            }
            root = root.child(result.build()).children(transcript);
        }
        root.build()
    }
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the progress line reads its row, seek, state and measured line, and handles drag and keys"
)]
fn progress_line(
    id: &str,
    theme: &ArtisanTheme,
    shown: &Shown,
    name: &str,
    seek: Seek,
    state: AudioState,
    line: Rc<Cell<Option<Rect>>>,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    let duration = shown.duration();
    let progress = shown.progress();
    let background = theme.colors.text.at(Opacity::Rule).to_reference_paint();
    let foreground = theme.colors.text_muted.to_reference_paint();
    let draw_line = Rc::clone(&line);
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
    if let Some(length) = duration {
        let position = shown.position();
        let key_seek = Rc::clone(&seek);
        let key_invalidate = Rc::clone(&invalidate);
        let request_seek = Rc::clone(&seek);
        let request_invalidate = Rc::clone(&invalidate);
        element = element
            .role(Role::Slider)
            .aria_label(format!("Seek {name}"))
            .aria_range(NumericRangeSemantics {
                min: 0.0,
                max: length.as_secs_f64(),
                value: position.as_secs_f64(),
                step: SEEK_STEP.as_secs_f64(),
            })
            .aria_value(format!("{} of {}", clock(position), clock(length)))
            .focusable(true)
            .focus_visible_style(focus_ring(theme))
            .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                    return InputResponse::default();
                };
                let Some(command) = SeekCommand::of_key(key.key) else {
                    return InputResponse::default();
                };
                key_seek(command.target(position, length));
                key_invalidate();
                InputResponse {
                    stop_propagation: true,
                    ..InputResponse::handled()
                }
            })
            .on_accessibility_request(move |request| {
                if let Some(command) = SeekCommand::of_request(request) {
                    request_seek(command.target(position, length));
                    request_invalidate();
                }
            });
        let pressing = state.clone();
        let press_id = id.to_owned();
        let press_seek = Rc::clone(&seek);
        let press_line = Rc::clone(&line);
        let press_invalidate = Rc::clone(&invalidate);
        let moving = state.clone();
        let move_id = id.to_owned();
        let move_seek = seek;
        let move_line = line;
        let move_invalidate = Rc::clone(&invalidate);
        let releasing = state;
        let release_id = id.to_owned();
        element = element
            .cursor(aqua_core::CursorIcon::Pointer)
            .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::PointerDown(pointer, PointerButton::Primary)) =
                    &event.input
                else {
                    return InputResponse::default();
                };
                seek_at(&press_seek, &press_line, duration, pointer.position.x);
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
                    seek_at(&move_seek, &move_line, duration, pointer.position.x);
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

enum Unplayable {
    Loading,
    Failed {
        problem: editor_core::Problem,
        retryable: bool,
    },
}

#[cfg(test)]
#[path = "../../../../../tests/ui/rich/media/audio.rs"]
mod tests;

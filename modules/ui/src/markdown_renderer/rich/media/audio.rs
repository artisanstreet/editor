//! `audio`: local audio files, each a row with a play button, a seekable
//! progress line, and its time.
//!
//! Playback is app-wide and one track at a time: [`AudioOutput`] holds the
//! app's one [`Player`], and starting a track stops any other. A row reads
//! the player's snapshot each frame and draws its state only while the
//! player holds its track; while that track plays, the row redraws a few
//! times a second so its time moves. Clicking or dragging the progress line
//! seeks. Only files play: a URL says so instead. A file fetched from the
//! Forge's machine, or a `data:` URL, plays from memory, as the bytes the
//! row first loaded: a later row naming a rewritten file is a new track.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use artisan_assets::AssetId;
use artisan_audio::{PlaybackState, Player, Snapshot};
use gpui::{
    AnyElement, App, Asset, Bounds, ElementId, Global, IntoElement, MouseButton,
    ParentElement as _, Pixels, RenderOnce, SharedString, Styled as _, Task, Window, canvas, div,
    prelude::*, px, relative,
};
use serde::Deserialize;

use super::super::kit::{self, Tone};
use super::super::{BlockContext, RichBlock};
use super::source::{Located, MediaFetcher, MediaRequest, Owned, locate};
use crate::theme::ArtisanTheme;

/// How often a playing row redraws its time.
const TICK: Duration = Duration::from_millis(200);

/// Edge of the play button.
const BUTTON: f32 = 28.0;

/// The app's one audio output, created on first use.
#[derive(Default)]
struct AudioOutput(Player);

impl Global for AudioOutput {}

impl AudioOutput {
    fn player(cx: &mut App) -> Player {
        cx.default_global::<Self>().0.clone()
    }
}

/// The `audio` spec: one `src`, or `tracks`.
#[derive(Debug, Deserialize)]
#[serde(try_from = "Spec")]
pub(super) struct AudioBlock {
    tracks: Vec<Track>,
}

#[derive(Debug, Deserialize)]
struct Spec {
    #[serde(default)]
    src: Option<String>,
    #[serde(default, alias = "label")]
    name: Option<String>,
    #[serde(default)]
    tracks: Vec<Track>,
}

#[derive(Clone, Debug, Deserialize)]
struct Track {
    src: String,
    #[serde(default, alias = "label")]
    name: Option<String>,
}

impl Track {
    /// The name as written, else the file name.
    fn shown_name(&self) -> SharedString {
        let named = self
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let file = || {
            let src = self.src.trim().trim_end_matches('/');
            src.rsplit(['/', '\\']).next().unwrap_or(src).to_owned()
        };
        SharedString::from(named.map_or_else(file, str::to_owned))
    }
}

impl TryFrom<Spec> for AudioBlock {
    type Error = String;

    fn try_from(spec: Spec) -> Result<Self, String> {
        let mut tracks = spec.tracks;
        if let Some(src) = spec.src {
            tracks.insert(
                0,
                Track {
                    src,
                    name: spec.name,
                },
            );
        }
        tracks.retain(|track| !track.src.trim().is_empty());
        if tracks.is_empty() {
            return Err("audio needs a `src` or `tracks`".to_owned());
        }
        Ok(Self { tracks })
    }
}

impl RichBlock for AudioBlock {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children(
                self.tracks
                    .iter()
                    .enumerate()
                    .map(|(index, track)| TrackRow {
                        id: cx.key(format_args!("track-{index}")),
                        theme: *cx.theme,
                        src: SharedString::from(track.src.trim().to_owned()),
                        name: track.shown_name(),
                    }),
            )
            .into_any_element()
    }

    fn markdown(&self) -> String {
        self.tracks
            .iter()
            .map(|track| format!("- [{}]({})", track.shown_name(), track.src.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ---------------------------------------------------------------------------
// View

#[derive(IntoElement)]
struct TrackRow {
    id: SharedString,
    theme: ArtisanTheme,
    src: SharedString,
    name: SharedString,
}

/// A row's window state: its redraw timer and its progress line's bounds.
#[derive(Default)]
struct RowState {
    ticking: Option<Task<()>>,
    line: Rc<Cell<Option<Bounds<Pixels>>>>,
    dragging: bool,
}

/// What the row shows of the player.
#[derive(Clone, Debug, PartialEq)]
enum Shown {
    /// The player holds another track, or none.
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
    Failed(SharedString),
}

impl Shown {
    fn of(snapshot: &Snapshot, path: &PathBuf) -> Self {
        if snapshot.track.as_ref() != Some(path) {
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
            PlaybackState::Failed(message) => Self::Failed(SharedString::from(message.clone())),
        }
    }

    /// The share of the track played, `0..=1`.
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

    fn duration(&self) -> Option<Duration> {
        match self {
            Self::Playing { duration, .. }
            | Self::Paused { duration, .. }
            | Self::Ended { duration } => Some(*duration),
            _ => None,
        }
    }
}

impl TrackRow {
    fn part(&self, part: &str) -> ElementId {
        ElementId::Name(SharedString::from(format!("{}-{part}", self.id)))
    }

    /// The row with a muted line in place of its player.
    fn notice(&self, theme: &ArtisanTheme, message: String) -> AnyElement {
        row()
            .child(kit::glyph(
                AssetId::TABLER_PLAYER_PLAY_FILLED,
                14.0,
                theme.colors.muted_foreground.to_paint(),
            ))
            .child(kit::body(theme, self.name.clone()).flex_shrink_0())
            .child(kit::muted(theme, message).truncate())
            .into_any_element()
    }
}

/// Audio bytes that come from neither a URL nor this machine's disk.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum AudioBytes {
    /// A `data:` URL's payload.
    Data(Arc<[u8]>),
    /// A file fetched from where the open thread's files live.
    Fetched(MediaRequest),
}

impl AudioBytes {
    /// The name the player knows this track by: never a file on this
    /// machine, but unique to the source and ending in its format. A fetched
    /// file is named for the row `owner` too, as each row plays the bytes it
    /// loaded.
    fn track(&self, owner: &str, bytes: &[u8]) -> PathBuf {
        match self {
            Self::Fetched(request) => {
                PathBuf::from(format!("forge/{}/{owner}/{}", request.scope, request.path))
            }
            Self::Data(data) => {
                use std::hash::{Hash as _, Hasher as _};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                data.hash(&mut hasher);
                let extension = audio_extension(bytes).unwrap_or("audio");
                PathBuf::from(format!("data/{:016x}.{extension}", hasher.finish()))
            }
        }
    }
}

/// Loads [`AudioBytes`], fetching them through the host when they name a
/// file. Bytes that are not a known audio format, or cannot be fetched,
/// yield `None`. They stay in memory: the Editor writes no files of its own.
enum AudioBytesLoader {}

impl Asset for AudioBytesLoader {
    type Source = Owned<AudioBytes>;
    type Output = Option<Arc<[u8]>>;

    fn load(
        Owned { source, .. }: Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let fetch = match &source {
            AudioBytes::Fetched(request) => cx
                .try_global::<MediaFetcher>()
                .map(|fetcher| fetcher.fetch(request.clone())),
            AudioBytes::Data(_) => None,
        };
        async move {
            let bytes = match source {
                AudioBytes::Data(bytes) => bytes,
                AudioBytes::Fetched(_) => fetch?.await?,
            };
            audio_extension(&bytes).is_some().then_some(bytes)
        }
    }
}

/// The file extension of the audio format `bytes` begin with.
fn audio_extension(bytes: &[u8]) -> Option<&'static str> {
    let starts = |prefix: &[u8]| bytes.starts_with(prefix);
    if starts(b"ID3") || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0) {
        Some("mp3")
    } else if bytes.len() >= 12 && starts(b"RIFF") && &bytes[8..12] == b"WAVE" {
        Some("wav")
    } else if starts(b"fLaC") {
        Some("flac")
    } else if starts(b"OggS") {
        Some("ogg")
    } else {
        None
    }
}

/// What a row plays.
struct Playable {
    /// The name the player knows the track by.
    path: PathBuf,
    /// The bytes it plays from memory, unless it is a file on this machine.
    bytes: Option<Arc<[u8]>>,
}

impl TrackRow {
    /// The row's track, else the notice the row shows instead.
    fn playable(&self, window: &mut Window, cx: &mut App) -> Result<Playable, String> {
        let source = match locate(&self.src, cx) {
            Located::Path(path) => return Ok(Playable { path, bytes: None }),
            Located::Data(bytes) => AudioBytes::Data(bytes.into()),
            Located::Fetched(request) => AudioBytes::Fetched(request),
            Located::Url(_) => return Err("Only local audio files play here".into()),
            Located::Invalid => return Err(format!("Couldn't load {}", self.src.trim())),
        };
        let owned = Owned::new(&self.id, source);
        match window.use_asset::<AudioBytesLoader>(&owned, cx) {
            Some(Some(bytes)) => Ok(Playable {
                path: owned.source.track(&self.id, &bytes),
                bytes: Some(bytes),
            }),
            None => Err("Loading…".into()),
            Some(None) => Err(format!("Couldn't load {}", self.src.trim())),
        }
    }
}

impl RenderOnce for TrackRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let Playable { path, bytes } = match self.playable(window, cx) {
            Ok(track) => track,
            Err(message) => return self.notice(&theme, message),
        };
        let player = AudioOutput::player(cx);
        let shown = Shown::of(&player.snapshot(), &path);
        let state = kit::keyed(window, cx, self.part("state"), RowState::default);
        keep_ticking(&state, &shown, cx);

        let button = {
            let player = player.clone();
            let path = path.clone();
            let shown = shown.clone();
            let state = state.clone();
            div()
                .id(self.part("button"))
                .size(px(BUTTON))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .bg(theme.colors.foreground.with_alpha(0.08).to_paint())
                .hover(|style| style.bg(theme.colors.foreground.with_alpha(0.14).to_paint()))
                .child(kit::glyph(
                    if matches!(shown, Shown::Playing { .. } | Shown::Loading) {
                        AssetId::TABLER_PLAYER_PAUSE_FILLED
                    } else {
                        AssetId::TABLER_PLAYER_PLAY_FILLED
                    },
                    12.0,
                    theme.colors.foreground.to_paint(),
                ))
                .on_click(move |_, _, cx| {
                    match shown {
                        Shown::Playing { .. } | Shown::Loading => player.pause(),
                        Shown::Paused { .. } | Shown::Ended { .. } => player.resume(),
                        Shown::Idle | Shown::Failed(_) => match &bytes {
                            Some(bytes) => player.play_bytes(&path, Arc::clone(bytes)),
                            None => player.play(&path),
                        },
                    }
                    // Redraw now, not at the next tick.
                    state.update(cx, |_, cx| cx.notify());
                })
        };

        let detail = match &shown {
            Shown::Failed(message) => kit::muted(&theme, message.clone())
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(Tone::Danger.color(&theme).to_paint())
                .into_any_element(),
            _ => self
                .progress_line(&state, &shown, &player, cx)
                .into_any_element(),
        };
        let time = match &shown {
            Shown::Playing { position, duration } | Shown::Paused { position, duration } => {
                Some(format!("{} / {}", clock(*position), clock(*duration)))
            }
            Shown::Ended { duration } => Some(clock(*duration)),
            Shown::Loading => Some("Loading…".to_owned()),
            Shown::Idle | Shown::Failed(_) => None,
        };

        row()
            .child(button)
            .child(
                kit::body(&theme, self.name.clone())
                    .flex_shrink_0()
                    .max_w(relative(0.4))
                    .truncate()
                    .font_weight(gpui::FontWeight::MEDIUM),
            )
            .child(detail)
            .children(time.map(|time| {
                kit::mono(&theme, time)
                    .flex_shrink_0()
                    .text_size(px(12.0))
                    .text_color(theme.colors.muted_foreground.to_paint())
            }))
            .into_any_element()
    }
}

impl TrackRow {
    /// The progress line: a hairline track filled to the share played.
    /// Pressing or dragging on it seeks.
    fn progress_line(
        &self,
        state: &gpui::Entity<RowState>,
        shown: &Shown,
        player: &Player,
        cx: &App,
    ) -> impl IntoElement {
        let theme = self.theme;
        let duration = shown.duration();
        let probe = Rc::clone(&state.read(cx).line);
        let seek = {
            let probe = Rc::clone(&probe);
            let player = player.clone();
            move |x: Pixels| {
                let (Some(bounds), Some(duration)) = (probe.get(), duration) else {
                    return;
                };
                let width = f32::from(bounds.size.width).max(1.0);
                let share = (f32::from(x - bounds.origin.x) / width).clamp(0.0, 1.0);
                player.seek(duration.mul_f32(share));
            }
        };
        let press = {
            let seek = seek.clone();
            let state = state.clone();
            move |event: &gpui::MouseDownEvent, _: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                seek(event.position.x);
                state.update(cx, |state, cx| {
                    state.dragging = true;
                    cx.notify();
                });
            }
        };
        let drag = {
            let state = state.clone();
            move |event: &gpui::MouseMoveEvent, _: &mut Window, cx: &mut App| {
                if state.read(cx).dragging && event.dragging() {
                    seek(event.position.x);
                    state.update(cx, |_, cx| cx.notify());
                }
            }
        };
        let release = {
            let state = state.clone();
            move |_: &gpui::MouseUpEvent, _: &mut Window, cx: &mut App| {
                state.update(cx, |state, cx| {
                    if std::mem::take(&mut state.dragging) {
                        cx.notify();
                    }
                });
            }
        };
        let seekable = duration.is_some();
        div()
            .id(self.part("line"))
            .flex_1()
            .min_w(px(48.0))
            .h(px(BUTTON))
            .flex()
            .items_center()
            .when(seekable, |line| {
                line.cursor_pointer()
                    .on_mouse_down(MouseButton::Left, press)
                    .on_mouse_move(drag)
                    .on_mouse_up(MouseButton::Left, release.clone())
                    .on_mouse_up_out(MouseButton::Left, release)
            })
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(4.0))
                    .rounded(px(2.0))
                    .bg(kit::hairline(&theme))
                    .child(
                        canvas(
                            move |bounds, _, _| probe.set(Some(bounds)),
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(relative(shown.progress()))
                            .rounded(px(2.0))
                            .bg(theme.colors.foreground.with_alpha(0.75).to_paint()),
                    ),
            )
    }
}

/// Keeps a timer redrawing the row while its track loads or plays, and
/// drops it otherwise.
fn keep_ticking(state: &gpui::Entity<RowState>, shown: &Shown, cx: &mut App) {
    let live = matches!(shown, Shown::Playing { .. } | Shown::Loading);
    if live == state.read(cx).ticking.is_some() {
        return;
    }
    let task = live.then(|| {
        let state = state.downgrade();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if state.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
    });
    state.update(cx, |state, _| state.ticking = task);
}

/// One row of the block.
fn row() -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(10.0))
        .min_h(px(BUTTON + 4.0))
}

/// `m:ss`, or `h:mm:ss` past an hour.
fn clock(time: Duration) -> String {
    let seconds = time.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_reads_minutes_and_hours() {
        assert_eq!(clock(Duration::from_secs(0)), "0:00");
        assert_eq!(clock(Duration::from_secs(75)), "1:15");
        assert_eq!(clock(Duration::from_secs(3_725)), "1:02:05");
    }

    #[test]
    fn a_track_is_named_by_its_file_unless_named() {
        let track = |src: &str, name: Option<&str>| Track {
            src: src.to_owned(),
            name: name.map(str::to_owned),
        };
        assert_eq!(
            track("out/take-2.wav", None).shown_name().as_ref(),
            "take-2.wav"
        );
        assert_eq!(track("a.mp3", Some("Intro")).shown_name().as_ref(), "Intro");
    }
}

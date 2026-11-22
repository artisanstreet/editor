use crate::conversation::compaction::{CompactionFrame, CompactionMotion, CompactionPhase};
use aqua_core::GpuiTextColor;
use aqua_ui::element::Element;
use artisan_api::ItemId;
use editor_ui::kit::TextRole;
use editor_ui::motion::MotionCurve;
use editor_ui::phrase::{PhraseMotion, PhraseOverflow, retained_phrase_or_text};
use editor_ui::sticker::{self, Sticker};
use editor_ui::theme::ArtisanTheme;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

const RISE: editor_ui::kit::Space = editor_ui::kit::Space::S4;

struct Chevron {
    from: f32,
    target: f32,
    started: Instant,
}

const SWAP: std::time::Duration = std::time::Duration::from_millis(150);
const ARRIVAL: std::time::Duration = std::time::Duration::from_millis(350);
const ARRIVAL_BLUR: f32 = 4.0;
const SWAP_RISE: f32 = 4.0;
const SWAP_BLUR: f32 = 2.0;

struct SwapState {
    label: Option<String>,
    started: Option<Instant>,
}

struct DisclosedState {
    open: bool,
    started: Option<Instant>,
    height: Rc<Cell<f32>>,
}

struct StickerState {
    started: Option<Instant>,
}

struct EntranceState {
    started: Option<Instant>,
    height: Rc<Cell<f32>>,
}

#[derive(Debug)]
pub struct EntranceFrame {
    grow: f32,
    reveal: f32,
    height: Rc<Cell<f32>>,
    settling: bool,
}

#[derive(Default)]
pub struct RowMotion {
    scope: Option<crate::conversation::presentation::PresentationScope>,
    phrases: HashMap<String, PhraseMotion>,
    chevrons: HashMap<String, Chevron>,
    compactions: HashMap<ItemId, CompactionMotion>,
    entrances: HashMap<crate::conversation::fold::RowKey, EntranceState>,
    stickers: HashMap<crate::conversation::fold::RowKey, StickerState>,
    disclosed: HashMap<String, DisclosedState>,
    swaps: HashMap<String, SwapState>,
    arrivals: HashMap<String, (bool, Option<Instant>)>,
    animating: bool,
    refit: Option<Rc<dyn Fn()>>,
}

impl std::fmt::Debug for RowMotion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowMotion").finish_non_exhaustive()
    }
}

impl RowMotion {
    pub(crate) fn rebind_scope(
        &mut self,
        scope: &crate::conversation::presentation::PresentationScope,
    ) {
        if self.scope.as_ref() != Some(scope) {
            self.phrases.clear();
            self.chevrons.clear();
            self.compactions.clear();
            self.entrances.clear();
            self.stickers.clear();
            self.disclosed.clear();
            self.swaps.clear();
            self.arrivals.clear();
            self.animating = false;
            self.scope = Some(scope.clone());
        }
    }

    pub(crate) fn scope_is(
        &self,
        scope: &crate::conversation::presentation::PresentationScope,
    ) -> bool {
        self.scope.as_ref() == Some(scope)
    }
    pub fn entrance(
        &mut self,
        key: crate::conversation::fold::RowKey,
        fresh: bool,
        live: bool,
        reduced_motion: bool,
        now: Instant,
    ) -> Option<EntranceFrame> {
        let state = self.entrances.entry(key).or_insert_with(|| EntranceState {
            started: (fresh && live && !reduced_motion).then_some(now),
            height: Rc::new(Cell::new(0.0)),
        });
        let started = state.started?;
        let height = Rc::clone(&state.height);
        self.entrance_frame(started, height, reduced_motion, now)
    }

    pub fn disclosed(
        &mut self,
        key: String,
        open: bool,
        reduced_motion: bool,
        now: Instant,
    ) -> Option<EntranceFrame> {
        let state = self.disclosed.entry(key).or_insert_with(|| DisclosedState {
            open,
            started: None,
            height: Rc::new(Cell::new(0.0)),
        });
        if open && !state.open && !reduced_motion {
            state.started = Some(now);
        }
        state.open = open;
        if !open {
            state.started = None;
        }
        let started = state.started?;
        let height = Rc::clone(&state.height);
        self.entrance_frame(started, height, reduced_motion, now)
    }

    pub fn swap(
        &mut self,
        key: String,
        label: Option<String>,
        reduced_motion: bool,
        now: Instant,
    ) -> Option<f32> {
        let state = self.swaps.entry(key).or_insert_with(|| SwapState {
            label: label.clone(),
            started: None,
        });
        if label.is_some() && state.label.is_some() && state.label != label && !reduced_motion {
            state.started = Some(now);
        }
        if label.is_none() {
            state.started = None;
        }
        state.label = label;
        let elapsed = now.saturating_duration_since(state.started?);
        if elapsed >= SWAP || reduced_motion {
            return None;
        }
        self.animating = true;
        Some(MotionCurve::EaseInOut.sample_f32(elapsed.as_secs_f64() / SWAP.as_secs_f64()))
    }

    pub fn arrival(
        &mut self,
        key: String,
        ready: bool,
        reduced_motion: bool,
        now: Instant,
    ) -> Option<f32> {
        let state = self.arrivals.entry(key).or_insert((ready, None));
        if ready && !state.0 && !reduced_motion {
            state.1 = Some(now);
        }
        state.0 = ready;
        let elapsed = now.saturating_duration_since(state.1?);
        if elapsed >= ARRIVAL || reduced_motion {
            return None;
        }
        self.animating = true;
        Some(MotionCurve::EaseInOut.sample_f32(elapsed.as_secs_f64() / ARRIVAL.as_secs_f64()))
    }

    fn entrance_frame(
        &mut self,
        started: Instant,
        height: Rc<Cell<f32>>,
        reduced_motion: bool,
        now: Instant,
    ) -> Option<EntranceFrame> {
        let elapsed = now.saturating_duration_since(started);
        if elapsed >= std::time::Duration::from_millis(400) || reduced_motion {
            return None;
        }
        self.animating = true;
        Some(EntranceFrame {
            grow: MotionCurve::SmoothOut.sample_f32(elapsed.as_secs_f64() / 0.25),
            reveal: MotionCurve::EaseOut.sample_f32(
                elapsed
                    .saturating_sub(std::time::Duration::from_millis(80))
                    .as_secs_f64()
                    / 0.25,
            ),
            height,
            settling: elapsed < std::time::Duration::from_millis(205),
        })
    }

    pub fn sticker(
        &mut self,
        key: crate::conversation::fold::RowKey,
        fresh: bool,
        reduced_motion: bool,
        now: Instant,
    ) -> Option<Sticker> {
        let state = self.stickers.entry(key).or_insert_with(|| StickerState {
            started: (fresh && !reduced_motion).then_some(now),
        });
        let elapsed = now.saturating_duration_since(state.started?);
        if elapsed >= sticker::DURATION || reduced_motion {
            return None;
        }
        Some(Sticker { elapsed })
    }

    #[must_use]
    pub(crate) fn sticking(&self, now: Instant) -> bool {
        self.stickers.values().any(|state| {
            state
                .started
                .is_some_and(|started| now.saturating_duration_since(started) < sticker::DURATION)
        })
    }

    pub(crate) fn begin_frame(&mut self) {
        self.animating = false;
    }

    pub fn on_refit(&mut self, refit: Rc<dyn Fn()>) {
        self.refit = Some(refit);
    }

    pub fn animating(&self) -> bool {
        self.animating
    }

    pub fn phrase(
        &mut self,
        key: &str,
        text: String,
        color: GpuiTextColor,
        theme: &ArtisanTheme,
        now: f64,
        reduced_motion: bool,
    ) -> Element {
        let phrase = self.phrases.entry(key.to_owned()).or_default();
        let (words, _) = phrase.words(&text, now, reduced_motion);
        let overflow = PhraseOverflow {
            fit: phrase.fit(),
            refit: self.refit.clone(),
        };
        let role = TextRole::Prose;
        retained_phrase_or_text(
            words.as_deref(),
            text,
            role.style(theme),
            color,
            role.line_px(theme),
            overflow,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the chevron reads its key, open state, drag fraction, size, colour and motion clock"
    )]
    pub(crate) fn chevron(
        &mut self,
        key: &str,
        open: bool,
        fraction: Option<f32>,
        size: f32,
        color: aqua_core::GpuiHsla,
        now: Instant,
        reduced_motion: bool,
    ) -> Element {
        let target = if open { 1.0 } else { 0.0 };
        let state = self.chevrons.entry(key.to_owned()).or_insert(Chevron {
            from: target,
            target,
            started: now,
        });
        let elapsed = now.saturating_duration_since(state.started).as_secs_f64();
        let progress = MotionCurve::SmoothOut.sample_f32(elapsed / 0.25);
        let current = (state.target - state.from).mul_add(progress, state.from);
        if state.target.to_bits() != target.to_bits() {
            state.from = current;
            state.target = target;
            state.started = now;
        }
        let value = if reduced_motion {
            state.from = target;
            target
        } else {
            current
        };
        self.animating |= !reduced_motion && (value - target).abs() > 0.001;
        editor_ui::icon::disclosure_chevron(fraction.unwrap_or(value), size, color)
    }

    pub(crate) fn compaction(
        &mut self,
        id: &ItemId,
        phase: CompactionPhase,
        fresh: bool,
        now: Instant,
        reduced_motion: bool,
    ) -> CompactionFrame {
        let motion = self
            .compactions
            .entry(id.clone())
            .or_insert_with(|| CompactionMotion::first_seen(phase, fresh, now));
        let (frame, moving) = motion.advance(phase, !reduced_motion, now);
        self.animating |= moving;
        frame
    }
}

impl EntranceFrame {
    pub const fn settling(&self) -> bool {
        self.settling
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the entrance collapses the gap above the row while it grows; the negative margin is motion, not spacing"
    )]
    pub fn wrap(self, element: Element, gap: f32, invalidate: Rc<dyn Fn()>) -> Element {
        use aqua_ui::element::{Box as UiBox, Canvas};
        use aqua_ui::style::{Length, Style};
        let measured = Rc::clone(&self.height);
        let probe = Canvas::new()
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .on_prepaint(move |tree, node| {
                let Some(parent) = tree.get(node).and_then(|record| record.parent) else {
                    return;
                };
                if let Some(bounds) = tree.pre_transform_rect(parent)
                    && measured.replace(bounds.h).to_bits() != bounds.h.to_bits()
                {
                    invalidate();
                }
            })
            .build()
            .exclude_from_hit_testing();
        let content = UiBox::new()
            .width(Length::Pct(100.0))
            .style(
                Style::new()
                    .flex_shrink(0.0)
                    .opacity(self.reveal)
                    .transform(aqua_core::Transform2D::translate(
                        0.0,
                        RISE.px() * (1.0 - self.reveal),
                    )),
            )
            .child(element)
            .child(probe)
            .build();
        UiBox::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .style(
                Style::new()
                    .height(Length::Px(self.height.get() * self.grow))
                    .clip(true)
                    .margin_insets(aqua_core::Insets {
                        top: -gap * (1.0 - self.grow),
                        ..Default::default()
                    }),
            )
            .child(content)
            .build()
    }
}

pub(crate) fn swapped(element: Element, progress: f32) -> Element {
    use aqua_ui::element::Box as UiBox;
    use aqua_ui::style::{Length, Style};
    UiBox::new()
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .opacity(progress)
                .content_blur(SWAP_BLUR * (1.0 - progress))
                .transform(aqua_core::Transform2D::translate(
                    0.0,
                    SWAP_RISE * (1.0 - progress),
                )),
        )
        .child(element)
        .build()
}

pub(crate) fn arrived(element: Element, progress: f32) -> Element {
    use aqua_ui::element::Box as UiBox;
    use aqua_ui::style::{Length, Style};
    UiBox::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .style(
            Style::new()
                .opacity(progress)
                .content_blur(ARRIVAL_BLUR * (1.0 - progress)),
        )
        .child(element)
        .build()
}

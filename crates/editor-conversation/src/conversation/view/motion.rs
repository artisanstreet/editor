use crate::conversation::compaction::{CompactionFrame, CompactionMotion, CompactionPhase};
use editor_ui::motion::MotionCurve;
use editor_ui::phrase::{PhraseMotion, PhraseOverflow, retained_phrase_or_text};
use editor_ui::kit::TextRole;
use editor_ui::theme::ArtisanTheme;
use aqua_core::GpuiTextColor;
use aqua_ui::element::Element;
use artisan_api::ItemId;
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

struct EntranceState {
    started: Option<Instant>,
    height: Rc<Cell<f32>>,
}

pub struct EntranceFrame {
    grow: f32,
    reveal: f32,
    height: Rc<Cell<f32>>,
    pub settling: bool,
}

#[derive(Default)]
pub struct RowMotion {
    scope: Option<crate::conversation::presentation::PresentationScope>,
    phrases: HashMap<String, PhraseMotion>,
    chevrons: HashMap<String, Chevron>,
    compactions: HashMap<ItemId, CompactionMotion>,
    entrances: HashMap<crate::conversation::fold::RowKey, EntranceState>,
    animating: bool,
    refit: Option<Rc<dyn Fn()>>,
}

impl RowMotion {
    pub fn rebind_scope(&mut self, scope: &crate::conversation::presentation::PresentationScope) {
        if self.scope.as_ref() != Some(scope) {
            self.phrases.clear();
            self.chevrons.clear();
            self.compactions.clear();
            self.entrances.clear();
            self.animating = false;
            self.scope = Some(scope.clone());
        }
    }

    pub fn scope_is(&self, scope: &crate::conversation::presentation::PresentationScope) -> bool {
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
            height: state.height.clone(),
            settling: elapsed < std::time::Duration::from_millis(205),
        })
    }

    pub fn begin_frame(&mut self) {
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
        theme: ArtisanTheme,
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
            role.style(&theme),
            color,
            role.spec().line,
            overflow,
        )
    }

    pub fn chevron(
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
        if state.target != target {
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

    pub fn compaction(
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

    pub fn retain(
        &mut self,
        phrases: impl Fn(&str) -> bool,
        compactions: impl Fn(&ItemId) -> bool,
    ) {
        self.phrases.retain(|key, _| phrases(key));
        self.chevrons.retain(|key, _| phrases(key));
        self.compactions.retain(|id, _| compactions(id));
    }
}

impl EntranceFrame {
    #[expect(
        clippy::disallowed_methods,
        reason = "the entrance collapses the gap above the row while it grows; the negative margin is motion, not spacing"
    )]
    pub fn wrap(self, element: Element, gap: f32, invalidate: Rc<dyn Fn()>) -> Element {
        use aqua_ui::element::{Box as UiBox, Canvas};
        use aqua_ui::style::{Length, Style};
        let measured = self.height.clone();
        let probe = Canvas::new()
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .on_prepaint(move |tree, node| {
                let Some(parent) = tree.get(node).and_then(|record| record.parent) else {
                    return;
                };
                if let Some(bounds) = tree.pre_transform_rect(parent)
                    && measured.replace(bounds.h) != bounds.h
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

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Key},
    style::Style,
};
use editor_assets::AssetId;

use super::{
    asset::source_asset_glyph,
    button::{Button, ButtonVariant},
    motion::MotionCurve,
    theme::ArtisanTheme,
};

const HOLD: Duration = Duration::from_millis(1_500);
const SWAP: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyEffect {
    Clipboard(String),
    RequestFrame,
    WakeAt(Instant),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CopyFrame {
    pub progress: f32,
    pub animating: bool,
    pub wake_at: Option<Instant>,
}

#[derive(Clone, Default)]
pub struct CodeCopyController {
    copied_at: Rc<Cell<Option<Instant>>>,
}

impl CodeCopyController {
    pub fn copied(&self, now: Instant) {
        self.copied_at.set(Some(now));
    }

    pub fn frame(&self, now: Instant, reduced_motion: bool) -> CopyFrame {
        let Some(copied_at) = self.copied_at.get() else {
            return CopyFrame::default();
        };
        let elapsed = now.saturating_duration_since(copied_at);
        if reduced_motion {
            return CopyFrame {
                progress: if elapsed < HOLD { 1.0 } else { 0.0 },
                animating: false,
                wake_at: (elapsed < HOLD).then_some(copied_at + HOLD),
            };
        }
        let unit = if elapsed < SWAP {
            elapsed.as_secs_f64() / SWAP.as_secs_f64()
        } else if elapsed < HOLD {
            1.0
        } else {
            1.0 - (elapsed.saturating_sub(HOLD).as_secs_f64() / SWAP.as_secs_f64()).min(1.0)
        };
        CopyFrame {
            progress: MotionCurve::EaseInOut.sample_f32(unit),
            animating: elapsed < SWAP || (elapsed >= HOLD && elapsed < HOLD + SWAP),
            wake_at: (elapsed < HOLD).then_some(copied_at + HOLD),
        }
    }
}

pub struct CodeCopyButton {
    id: Key,
    source: String,
    theme: ArtisanTheme,
    controller: CodeCopyController,
    reduced_motion: bool,
    effect: Rc<dyn Fn(CopyEffect)>,
}

impl CodeCopyButton {
    pub fn new(
        id: impl Into<Key>,
        source: impl Into<String>,
        theme: ArtisanTheme,
        controller: CodeCopyController,
        reduced_motion: bool,
        effect: impl Fn(CopyEffect) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            theme,
            controller,
            reduced_motion,
            effect: Rc::new(effect),
        }
    }

    pub fn build(self, now: Instant) -> Element {
        let progress = self.controller.frame(now, self.reduced_motion).progress;
        let mut icon = UiBox::new().width(16.0).height(16.0);
        let tint = self.theme.colors.text_muted.to_reference_paint();
        let face = |asset, opacity: f32| {
            let mut style = Style::new()
                .absolute_at(Some(0.0), Some(0.0), None, None)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .opacity(opacity);
            if opacity < 1.0 {
                style = style.content_blur(2.0 * (1.0 - opacity));
            }
            UiBox::new()
                .width(16.0)
                .height(16.0)
                .style(style)
                .child(source_asset_glyph(
                    asset,
                    16.0 * 0.75_f32.mul_add(opacity, 0.25),
                    tint,
                ))
                .build()
        };
        if progress < 1.0 {
            icon = icon.child(face(AssetId::TABLER_COPY, 1.0 - progress));
        }
        if progress > 0.0 {
            icon = icon.child(face(AssetId::TABLER_CHECK, progress));
        }
        let controller = self.controller;
        let effect = self.effect;
        let source = self.source;
        Button::icon(self.id, self.theme, AssetId::TABLER_COPY, "Copy code")
            .variant(ButtonVariant::Ghost)
            .icon_slot(icon.build())
            .on_activate(move |_| {
                let now = Instant::now();
                controller.copied(now);
                effect(CopyEffect::Clipboard(source.clone()));
                effect(CopyEffect::RequestFrame);
                effect(CopyEffect::WakeAt(now + HOLD));
            })
            .build()
    }
}

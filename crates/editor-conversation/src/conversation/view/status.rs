use crate::conversation::adapter::RowModel;
use crate::conversation::phase::{self, Phase};
use editor_ui::theme::ArtisanTheme;
use aqua_ui::element::Element;
use artisan_api::{LiveTarget, RunId};
use std::collections::HashMap;

use crate::conversation::fold::RowKey;
use editor_ui::shimmer_text::{ShimmerMotion, ShimmerText, ShimmerTiming, ShimmerTone};
use aqua_text::{FontFamily, TextStyle};
use aqua_ui::element::Key;
use aqua_ui::tree::hash_key;

pub enum StatusText {
    Reasoning(String),
    Status(String),
}

struct StatusShimmer {
    key: String,
    motion: ShimmerMotion,
    timing: ShimmerTiming,
}

#[derive(Default)]
pub struct StatusShimmers {
    entries: HashMap<RowKey, StatusShimmer>,
    scope: Option<crate::conversation::presentation::PresentationScope>,
}

impl StatusShimmers {
    pub fn set_scope(&mut self, scope: &crate::conversation::presentation::PresentationScope) {
        if self.scope.as_ref() != Some(scope) {
            self.entries.clear();
            self.scope = Some(scope.clone());
        }
    }
    pub fn render(
        &mut self,
        row: RowKey,
        text: StatusText,
        live: bool,
        theme: ArtisanTheme,
    ) -> anyhow::Result<Element> {
        let (content, timing, runs) = match text {
            StatusText::Status(text) => (text, ShimmerTiming::Status, None),
            StatusText::Reasoning(text) => {
                let runs = editor_ui::inline_code_text::inline_runs(&text);
                (runs.text.clone(), ShimmerTiming::Progress, Some(runs))
            }
        };
        let key = format!("conversation-status-{:?}-{row:?}", self.scope);
        let entry = self.entries.entry(row).or_insert_with(|| StatusShimmer {
            key: key.clone(),
            motion: ShimmerMotion::new(&theme, timing),
            timing,
        });
        if entry.timing != timing {
            entry.motion = ShimmerMotion::new(&theme, timing);
            entry.timing = timing;
        }
        entry.motion.set_theme(&theme);
        entry.motion.set_active(live);
        let typography = TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: editor_ui::theme::ProseTypography::BODY_SIZE_PX,
            line_height_px: editor_ui::theme::ProseTypography::BODY_LINE_PX,
            weight: editor_ui::theme::ProseTypography::BODY_WEIGHT,
            ..TextStyle::default()
        };
        let spans = runs.map_or_else(Vec::new, |runs| {
            runs.styled_spans(
                &typography,
                theme.colors.text_muted.to_reference_text_color(),
                &theme,
            )
        });
        Ok(ShimmerText::new(key, content, theme)
            .active(live)
            .timing(timing)
            .tone(ShimmerTone::Muted)
            .truncate()
            .spans(spans)
            .build(typography))
    }

    pub fn agent(
        &mut self,
        row: RowKey,
        text: &'static str,
        theme: ArtisanTheme,
    ) -> anyhow::Result<Element> {
        self.render(row, StatusText::Status(text.to_owned()), true, theme)
    }

    pub fn sync(&self, app: &mut aqua::UiApp, reduced_motion: bool) -> bool {
        let mut changed = false;
        for entry in self.entries.values() {
            let key = hash_key(Key::from(entry.key.clone()));
            let node = app.tree().nodes_with_key(key).into_iter().next();
            if let Some(node) = node {
                changed |= entry.motion.sync(app, node, reduced_motion);
            }
        }
        changed
    }

    pub fn retain(&mut self, mut retained: impl FnMut(&RowKey) -> bool) {
        self.entries.retain(|key, _| retained(key));
    }
}

pub fn run<T>(
    model: &RowModel,
    run: &RunId,
    theme: ArtisanTheme,
    render: impl FnOnce(StatusText, bool, ArtisanTheme) -> T,
) -> Option<T> {
    let phase = model
        .run
        .as_ref()
        .map_or(Phase::Thinking, |summary| summary.phase);
    if phase == Phase::Compacting {
        return None;
    }
    let live_text = |target: LiveTarget| {
        model
            .live
            .get(&target)
            .map(|text| text.chars().take(2048).collect::<String>())
    };
    let reasoning = live_text(LiveTarget::Reasoning {
        run_id: run.clone(),
    })
    .filter(|_| phase == Phase::Thinking)
    .and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    });
    let text = match reasoning {
        Some(text) => StatusText::Reasoning(text),
        None => StatusText::Status(
            live_text(LiveTarget::Status {
                run_id: run.clone(),
            })
            .filter(|text| !text.trim().is_empty())
            .or_else(|| phase::status_copy(phase).map(str::to_owned))?,
        ),
    };
    let live = model.run.as_ref().is_none_or(|summary| summary.live());
    Some(render(text, live, theme))
}

pub fn agent<T>(
    working: bool,
    theme: ArtisanTheme,
    render: impl FnOnce(&'static str, ArtisanTheme) -> T,
) -> Option<T> {
    let text = phase::status_copy(if working {
        Phase::Working
    } else {
        Phase::Thinking
    })?;
    Some(render(text, theme))
}

use crate::conversation::adapter::RowModel;
use crate::conversation::phase::{self, Phase};
use editor_ui::theme::ArtisanTheme;
use aqua_ui::element::Element;
use artisan_api::{LiveTarget, RunId};

use crate::conversation::fold::RowKey;
use editor_ui::shimmer_text::{ShimmerText, ShimmerTiming, ShimmerTone};
use editor_ui::kit::TextRole;

pub enum StatusText {
    Reasoning(String),
    Status(String),
}

#[derive(Default)]
pub struct StatusShimmers {
    scope: Option<crate::conversation::presentation::PresentationScope>,
}

impl StatusShimmers {
    pub fn set_scope(&mut self, scope: &crate::conversation::presentation::PresentationScope) {
        self.scope = Some(scope.clone());
    }
    pub fn render(
        &mut self,
        row: RowKey,
        text: StatusText,
        live: bool,
        reduced_motion: bool,
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
        let typography = TextRole::Prose.style(&theme);
        let spans = runs.map_or_else(Vec::new, |runs| {
            runs.styled_spans(
                &typography,
                theme.colors.text_muted.to_reference_text_color(),
                &theme,
            )
        });
        Ok(ShimmerText::new(key, content, theme)
            .active(live)
            .reduced_motion(reduced_motion)
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
        reduced_motion: bool,
        theme: ArtisanTheme,
    ) -> anyhow::Result<Element> {
        self.render(
            row,
            StatusText::Status(text.to_owned()),
            true,
            reduced_motion,
            theme,
        )
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

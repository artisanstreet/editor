use super::{Dispatch, compaction, notice, request, status, work};
use crate::conversation::adapter::RowModel;
use crate::conversation::compaction::{CompactionFrame, CompactionPhase};
use crate::conversation::content::SourceField;
use crate::conversation::disclosure::RowDisclosure;
use crate::conversation::fold::{Row, Slot};
use editor_ui::theme::ArtisanTheme;
use aqua_ui::element::Element;
use artisan_api::{Collection, CollectionRef, Item, ItemId, ModelSwitch, Question, Timestamp};
use std::collections::HashMap;
use std::rc::Rc;

pub trait RowResources {
    fn validate(&self) -> anyhow::Result<()>;
    fn text(&self, model: &RowModel, slot: Slot, theme: ArtisanTheme) -> anyhow::Result<Element>;
    fn source(
        &self,
        item: &Item,
        field: SourceField,
        theme: ArtisanTheme,
    ) -> anyhow::Result<Element>;
    fn stored_questions(
        &self,
        item: &Item,
        source: &CollectionRef,
        theme: ArtisanTheme,
    ) -> anyhow::Result<Element>;
    fn answer(&self, item: &Item, theme: ArtisanTheme) -> anyhow::Result<Element>;
    fn answered_question(
        &self,
        item: &Item,
        questions: &Collection<Question>,
        theme: ArtisanTheme,
    ) -> anyhow::Result<Element> {
        request::answered(
            questions,
            theme,
            |field| self.source(item, field, theme),
            |source| self.stored_questions(item, source, theme),
            || self.answer(item, theme),
        )
    }
    fn image(
        &self,
        item: &Item,
        theme: ArtisanTheme,
        dispatch: Dispatch,
    ) -> anyhow::Result<Element>;
    fn phrase(
        &self,
        key: &str,
        text: String,
        color: aqua_core::GpuiTextColor,
        theme: ArtisanTheme,
    ) -> Element;
    fn chevron(
        &self,
        key: &str,
        open: bool,
        fraction: Option<f32>,
        size: f32,
        color: aqua_core::GpuiHsla,
    ) -> Element;
    fn truncate(
        &self,
        text: String,
        color: aqua_core::GpuiTextColor,
        mono: bool,
        theme: ArtisanTheme,
    ) -> Element;
    fn run_status(&self, text: status::StatusText, live: bool, theme: ArtisanTheme) -> Element;
    fn agent_status(&self, text: &'static str, theme: ArtisanTheme) -> Element;
    fn compaction_frame(&self, item: &Item) -> CompactionFrame;
    fn compaction_label(
        &self,
        phase: CompactionPhase,
        switch: Option<&ModelSwitch>,
        frame: CompactionFrame,
        theme: ArtisanTheme,
    ) -> Element;
}

pub fn render(
    model: &RowModel,
    agents: &HashMap<ItemId, String>,
    now: Timestamp,
    disclosure: RowDisclosure,
    theme: ArtisanTheme,
    resources: &impl RowResources,
    dispatch: Dispatch,
) -> anyhow::Result<Option<Element>> {
    resources.validate()?;
    let item = |id: &ItemId| {
        model
            .items
            .get(id)
            .map(std::sync::Arc::as_ref)
            .ok_or_else(|| anyhow::anyhow!("conversation row item {id} is unavailable"))
    };
    let prompt = |entry: &Item| -> anyhow::Result<Option<Element>> {
        if matches!(entry.kind, artisan_api::ItemKind::Question { .. }) {
            request::question(
                entry,
                theme,
                |field| resources.source(entry, field, theme),
                |source| resources.stored_questions(entry, source, theme),
                |item, questions| resources.answered_question(item, questions, theme),
            )
        } else {
            let ui = model.ui.get(&entry.id);
            let approval = request::approval(
                entry,
                ui.is_some_and(|ui| ui.sending),
                ui.and_then(|ui| ui.error.as_deref()),
                theme,
                |field| resources.source(entry, field, theme),
                Rc::clone(&dispatch),
            )?;
            if approval.is_some() {
                return Ok(approval);
            }
            let expanded = ui.and_then(|ui| ui.expanded).unwrap_or(false);
            Ok(match &entry.kind {
                artisan_api::ItemKind::Tool { .. }
                | artisan_api::ItemKind::UserMessage {
                    from_agent: Some(_),
                    ..
                } => work::activity_row(
                    entry,
                    expanded,
                    agents,
                    theme,
                    |text, color, mono| resources.truncate(text, color, mono, theme),
                    Rc::clone(&dispatch),
                ),
                artisan_api::ItemKind::Agent { .. } => work::agent(
                    entry,
                    theme,
                    |text, color| resources.truncate(text, color, false, theme),
                    Rc::clone(&dispatch),
                ),
                artisan_api::ItemKind::Plan { .. } => work::plan_entry(entry, theme),
                artisan_api::ItemKind::Notice { .. } => notice::render(
                    entry,
                    theme,
                    |text, color| resources.truncate(text, color, false, theme),
                    |field| resources.source(entry, field, theme),
                )?,
                _ => None,
            })
        }
    };
    let output = match &model.row {
        Row::Text { slot, .. } => Some(resources.text(model, *slot, theme)?),
        Row::Projected { row, .. } => match row.as_ref() {
            Row::Text { slot, .. } => Some(resources.text(model, *slot, theme)?),
            _ => None,
        },
        Row::Work {
            run,
            segment,
            entries,
            unloaded,
        } => {
            if *segment != 0 || model.run.is_none() {
                None
            } else {
                let summary = model.run.as_ref().expect("summary checked");
                let key = format!(
                    "work-{}-label",
                    run.as_ref().map_or_else(String::new, ToString::to_string)
                );
                let chevron_key = key.strip_suffix("-label").unwrap_or(&key);
                let label = resources.phrase(
                    &key,
                    work::section_title(summary, now),
                    theme.colors.text_muted.to_reference_text_color(),
                    theme,
                );
                work::header(
                    run.as_ref(),
                    *segment,
                    !entries.is_empty() || !unloaded.is_empty(),
                    Some(summary),
                    theme,
                    label,
                    |open| {
                        resources.chevron(
                            chevron_key,
                            open,
                            disclosure.fraction,
                            16.0,
                            theme.colors.text_muted.to_reference_paint(),
                        )
                    },
                    Rc::clone(&dispatch),
                )
            }
        }
        Row::EarlierWork { .. } | Row::Gap => {
            super::load_row(&model.row, theme, Rc::clone(&dispatch))
        }
        Row::WorkEntry { id, .. } => {
            let entry = item(id)?;
            match &entry.kind {
                artisan_api::ItemKind::Agent { .. } => work::agent(
                    entry,
                    theme,
                    |text, color| resources.truncate(text, color, false, theme),
                    Rc::clone(&dispatch),
                ),
                artisan_api::ItemKind::Tool { .. }
                | artisan_api::ItemKind::UserMessage {
                    from_agent: Some(_),
                    ..
                } => {
                    let expanded = model.ui.get(id).and_then(|ui| ui.expanded).unwrap_or(false);
                    work::activity_row(
                        entry,
                        expanded,
                        agents,
                        theme,
                        |text, color, mono| resources.truncate(text, color, mono, theme),
                        Rc::clone(&dispatch),
                    )
                }
                artisan_api::ItemKind::Plan { .. } => work::plan_entry(entry, theme),
                artisan_api::ItemKind::Notice { .. } => notice::render(
                    entry,
                    theme,
                    |text, color| resources.truncate(text, color, false, theme),
                    |field| resources.source(entry, field, theme),
                )?,
                artisan_api::ItemKind::Question { .. } => prompt(entry)?,
                _ => None,
            }
        }
        Row::Chain { ids, .. } => {
            let open = ids
                .first()
                .and_then(|id| model.ui.get(id))
                .and_then(|ui| ui.chain_open)
                .unwrap_or(false);
            let key = ids
                .first()
                .map_or_else(String::new, |id| format!("chain-{id}-clause"));
            let chevron_key = key.strip_suffix("-clause").unwrap_or(&key);
            work::chain(
                ids,
                &model.items,
                open,
                theme,
                |text, color| resources.phrase(&key, text, color, theme),
                |open, color| {
                    resources.chevron(chevron_key, open, disclosure.fraction, 14.0, color)
                },
                Rc::clone(&dispatch),
            )
        }
        Row::Prompt(id) => prompt(item(id)?)?,
        Row::Compaction(id) => {
            let entry = item(id)?;
            compaction::row(
                entry,
                resources.compaction_frame(entry),
                theme,
                |phase, switch, frame, theme| {
                    resources.compaction_label(phase, switch, frame, theme)
                },
            )
        }
        Row::Image(id) => Some(resources.image(item(id)?, theme, Rc::clone(&dispatch))?),
        Row::Status(run) => status::run(model, run, theme, |text, live, theme| {
            resources.run_status(text, live, theme)
        }),
        Row::AgentResult(_) => None,
        Row::AgentStatus(_, working) => status::agent(*working, theme, |text, theme| {
            resources.agent_status(text, theme)
        }),
    };
    Ok(output)
}

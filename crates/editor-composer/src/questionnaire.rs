//! Agent questions answered from the composer card.
//!
//! When an agent asks, the card extends upward with the open questionnaires
//! stacked above the editor: the one being answered in full, the others as
//! one-line rows beneath it that bring themselves forward when clicked. A
//! question shows its suggested answers as rows, and the editor below is
//! where an answer of your own is typed: while a questionnaire is open,
//! sending records the typed text as the current question's answer instead
//! of sending a message.
//!
//! Opening a questionnaire puts the draft text aside and
//! clears the editor for the answer. The aside draft stays Forge's draft
//! throughout: answer keystrokes are not draft changes, so nothing
//! overwrites it. It comes back once the last open questionnaire closes or
//! the composer moves to another thread.
//!
//! Questions stay open until answered. Dismissing a questionnaire skips all
//! its questions.

use crate::Composer;
use artisan_api::{Answer, Collection, ItemId, Question, QuestionAnswer};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::button::Button;
use editor_ui::theme::{
    ActiveTheme as _, ArtisanTheme, ColorTokens, RadiusStep, RadiusTokens, TextStep,
    TypographyTokens,
};
use gpui::StatefulInteractiveElement as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, div, px,
};
use std::collections::{BTreeMap, HashMap};

/// One open questionnaire: the questions one agent request asked together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerQuestionnaire {
    /// The question item the answer goes to.
    pub item: ItemId,
    pub questions: Collection<Question>,
}

/// The draft put aside while an answer is written.
struct DraftAside {
    text: String,
}

#[derive(Clone, Default)]
struct AnswerDraft {
    step: usize,
    recorded: BTreeMap<usize, QuestionAnswer>,
    ticked: Vec<String>,
    text: String,
    error: Option<String>,
}

/// The composer's questionnaire state.
#[derive(Default)]
pub(crate) struct QuestionnairePanel {
    /// Open questionnaires, oldest first, as the conversation last pushed.
    questionnaires: Vec<ComposerQuestionnaire>,
    /// Answered here and accepted by Forge, hidden until the item settles
    /// and leaves the pushed list.
    answered: Vec<ItemId>,
    /// The questionnaire being answered; the oldest when unset.
    active: Option<ItemId>,
    /// The question being answered in the active questionnaire.
    step: usize,
    /// Answers recorded so far, by question index; absent indices skip.
    recorded: BTreeMap<usize, QuestionAnswer>,
    /// Opaque option IDs selected on the current multi-select question.
    ticked: Vec<String>,
    aside: Option<DraftAside>,
    /// The questionnaire whose answer is in flight.
    sending: Option<ItemId>,
    /// Why the last answer failed, shown until the next attempt.
    error: Option<String>,
    drafts: HashMap<ItemId, AnswerDraft>,
    source: Option<gpui::Entity<editor_core::ContentText>>,
    payloads: HashMap<ItemId, Vec<Option<QuestionAnswer>>>,
    question_window: Option<gpui::Entity<editor_core::CollectionWindow<Question>>>,
    question_events: Option<gpui::Subscription>,
    question_owner: Option<ItemId>,
    options: Option<gpui::Entity<editor_core::CollectionBrowser<artisan_api::QuestionOption>>>,
    options_slot: Option<(ItemId, usize, artisan_api::CollectionRef)>,
}

impl QuestionnairePanel {
    pub(crate) fn sending(&self) -> bool {
        self.sending.is_some()
    }
    fn visible(&self) -> impl Iterator<Item = &ComposerQuestionnaire> {
        self.questionnaires
            .iter()
            .filter(|questionnaire| !self.answered.contains(&questionnaire.item))
    }

    fn active_questionnaire(&self) -> Option<&ComposerQuestionnaire> {
        match &self.active {
            Some(active) => self
                .visible()
                .find(|questionnaire| &questionnaire.item == active)
                .or_else(|| self.visible().next()),
            None => self.visible().next(),
        }
    }

    fn current_question<'a>(&'a self, cx: &'a gpui::App) -> Option<&'a Question> {
        let active = self.active_questionnaire()?;
        match &active.questions {
            Collection::Inline(questions) => questions.get(self.step),
            Collection::Stored(source) => {
                if self.question_owner.as_ref() != Some(&active.item) {
                    return None;
                }
                let window = self.question_window.as_ref()?.read(cx);
                if window.source() != source {
                    return None;
                }
                let page = window.page_for(self.step as u64)?;
                page.records.get(self.step - page.start as usize)
            }
        }
    }

    fn reset_progress(&mut self) {
        self.source = None;
        self.question_window = None;
        self.question_events = None;
        self.question_owner = None;
        self.options = None;
        self.options_slot = None;
        self.step = 0;
        self.recorded.clear();
        self.ticked.clear();
    }

    pub(crate) fn restore_aside_text(&mut self, text: &str) -> bool {
        if let Some(aside) = &mut self.aside {
            aside.text = text.to_owned();
            true
        } else {
            false
        }
    }
    /// The draft text Forge keeps while an answer is written.
    pub(crate) fn aside_text(&self) -> Option<&str> {
        self.aside.as_ref().map(|aside| aside.text.as_str())
    }
}

impl Composer {
    fn sync_question_window(&mut self, cx: &mut Context<Self>) {
        let source = self
            .questionnaire
            .active_questionnaire()
            .and_then(|active| {
                active
                    .questions
                    .stored()
                    .map(|source| (active.item.clone(), source.clone()))
            });
        let Some((id, source)) = source else {
            self.questionnaire.question_window = None;
            self.questionnaire.question_events = None;
            self.questionnaire.question_owner = None;
            return;
        };
        if self.questionnaire.question_owner.as_ref() != Some(&id) {
            let Some(host) = self.host(cx) else { return };
            let window = cx.new(|cx| editor_core::CollectionWindow::new(&host, source.clone(), cx));
            self.questionnaire.question_events = Some(cx.subscribe(
                &window,
                |_, _, _: &editor_core::CollectionWindowEvent, cx| cx.notify(),
            ));
            self.questionnaire.question_window = Some(window);
            self.questionnaire.question_owner = Some(id);
        }
        if let Some(window) = &self.questionnaire.question_window {
            let index = self.questionnaire.step as u64;
            window.update(cx, |window, cx| {
                window.set_source(source, cx);
                if window.page_for(index).is_none() {
                    window.request(index, cx);
                }
            });
        }
    }
    fn sync_options_browser(&mut self, question: &Question, cx: &mut Context<Self>) {
        let Some(source) = question.options.stored().cloned() else {
            self.questionnaire.options = None;
            self.questionnaire.options_slot = None;
            return;
        };
        let Some(item) = self
            .questionnaire
            .active_questionnaire()
            .map(|entry| entry.item.clone())
        else {
            return;
        };
        let slot = (item.clone(), self.questionnaire.step, source.clone());
        if self.questionnaire.options_slot.as_ref() == Some(&slot) {
            return;
        }
        let Some(host) = self.host(cx) else { return };
        let owner = cx.entity().downgrade();
        let preview_owner = owner.clone();
        let step = self.questionnaire.step;
        let multi = question.multi_select;
        let expected = source.clone();
        let expected_item = item.clone();
        let present = std::rc::Rc::new(
            move |option: &artisan_api::QuestionOption, _, cx: &gpui::App| {
                editor_core::CollectionPreview {
                    label: option.label.clone().into(),
                    detail: option.description.clone(),
                    disabled: preview_owner
                        .upgrade()
                        .is_none_or(|owner| owner.read(cx).questionnaire.sending.is_some()),
                    selected: preview_owner.upgrade().is_some_and(|owner| {
                        owner.read(cx).questionnaire.ticked.contains(&option.id)
                    }),
                }
            },
        );
        let activate = std::rc::Rc::new(
            move |option: &artisan_api::QuestionOption, _, cx: &mut gpui::App| {
                let id = option.id.clone();
                let _ = owner.update(cx, |this, cx| {
                    if this.questionnaire.step != step
                        || this
                            .questionnaire
                            .active_questionnaire()
                            .is_none_or(|active| active.item != expected_item)
                        || this
                            .questionnaire
                            .current_question(cx)
                            .is_none_or(|question| question.options.stored() != Some(&expected))
                        || this.questionnaire.sending.is_some()
                    {
                        return;
                    }
                    if multi {
                        this.toggle_suggested_answer(id, cx)
                    } else {
                        this.clear_answer_text(cx);
                        this.answer_current(
                            QuestionAnswer::Choices {
                                ids: vec![id].into(),
                            },
                            cx,
                        );
                    }
                });
            },
        );
        self.questionnaire.options = Some(cx.new(|cx| {
            editor_core::CollectionBrowser::new(
                &host,
                source,
                "Choices",
                *cx.theme(),
                present,
                Some(activate),
                cx,
            )
        }));
        self.questionnaire.options_slot = Some(slot);
    }
    fn open_question_source(
        &mut self,
        source: artisan_api::ContentRef,
        theme: ArtisanTheme,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.host(cx) else {
            return;
        };
        self.questionnaire.source =
            Some(cx.new(|cx| editor_core::ContentText::new(&host, source, theme, cx)));
        cx.notify();
    }

    /// Replaces the open questionnaires. A questionnaire no longer open
    /// drops its progress; the draft goes aside while any is open and comes
    /// back once none is.
    pub fn set_questionnaires(
        &mut self,
        questionnaires: Vec<ComposerQuestionnaire>,
        cx: &mut Context<Self>,
    ) {
        if self.questionnaire.questionnaires == questionnaires {
            return;
        }
        self.save_answer_progress(cx);
        let active_before = self
            .questionnaire
            .active_questionnaire()
            .map(|questionnaire| questionnaire.item.clone());
        let open = |item: &ItemId| {
            questionnaires
                .iter()
                .any(|questionnaire| &questionnaire.item == item)
        };
        self.questionnaire.answered.retain(|item| open(item));
        self.questionnaire.drafts.retain(|item, _| open(item));
        self.questionnaire.payloads.retain(|item, _| open(item));
        if self
            .questionnaire
            .sending
            .as_ref()
            .is_some_and(|item| !open(item))
        {
            self.questionnaire.sending = None;
        }
        self.questionnaire.questionnaires = questionnaires;
        let active_after = self
            .questionnaire
            .active_questionnaire()
            .map(|questionnaire| questionnaire.item.clone());
        if active_before != active_after {
            self.questionnaire.active = active_after;
            self.load_answer_progress(cx);
        }
        self.sync_answer_mode(cx);
        self.sync_question_window(cx);
        self.sync_editor_enabled(cx);
        cx.notify();
    }

    /// Whether the editor is the answer field: sending records the typed
    /// text as the current question's answer.
    pub(crate) fn writing_answer(&self) -> bool {
        self.questionnaire.aside.is_some()
    }

    /// Forgets every questionnaire and brings the draft back (the composer
    /// moved to another thread).
    pub(crate) fn leave_questionnaires(&mut self) {
        self.questionnaire.questionnaires.clear();
        self.questionnaire.answered.clear();
        self.questionnaire.active = None;
        self.questionnaire.sending = None;
        self.questionnaire.error = None;
        self.questionnaire.reset_progress();
        self.questionnaire.aside = None;
        self.questionnaire.drafts.clear();
        self.questionnaire.payloads.clear();
    }

    fn sync_answer_mode(&mut self, cx: &mut Context<Self>) {
        let open = self.questionnaire.active_questionnaire().is_some();
        match (open, self.questionnaire.aside.is_some()) {
            (true, false) => self.put_draft_aside(cx),
            (false, true) => self.return_draft_aside(cx),
            _ => {}
        }
    }

    /// Clears a partly typed answer; the draft aside is untouched.
    fn clear_answer_text(&mut self, cx: &mut Context<Self>) {
        if self.writing_answer() && self.has_text {
            self.editor.update(cx, |editor, cx| editor.set_text("", cx));
            self.has_text = false;
        }
    }

    fn put_draft_aside(&mut self, cx: &mut Context<Self>) {
        if self.questionnaire.aside.is_some() {
            return;
        }
        let text = self.editor.read(cx).text().to_owned();
        self.questionnaire.aside = Some(DraftAside { text });
        self.editor.update(cx, |editor, cx| editor.set_text("", cx));
        self.has_text = false;
    }

    /// Brings the draft back, keeping any image added meanwhile.
    fn return_draft_aside(&mut self, cx: &mut Context<Self>) {
        let Some(aside) = self.questionnaire.aside.take() else {
            return;
        };
        self.editor
            .update(cx, |editor, cx| editor.set_text(&aside.text, cx));
        self.has_text = !aside.text.trim().is_empty();
    }

    /// Records a free-text answer. Suggested choices have their own submit action.
    pub(crate) fn submit_written_answer(&mut self, cx: &mut Context<Self>) {
        let answer = self.editor.read(cx).text().trim().to_owned();
        if answer.is_empty() || self.questionnaire.sending.is_some() {
            return;
        }
        self.clear_answer_text(cx);
        self.answer_current(QuestionAnswer::Text { text: answer }, cx);
    }

    fn answer_current(&mut self, answer: QuestionAnswer, cx: &mut Context<Self>) {
        if self.questionnaire.current_question(cx).is_none() {
            return;
        }
        let step = self.questionnaire.step;
        self.questionnaire.error = None;
        self.questionnaire.recorded.insert(step, answer);
        self.questionnaire.ticked.clear();
        let count = self
            .questionnaire
            .active_questionnaire()
            .map_or(0, |questionnaire| questionnaire.questions.len() as usize);
        if step + 1 < count {
            self.questionnaire.source = None;
            self.questionnaire.step += 1;
            self.sync_question_window(cx);
            cx.notify();
            return;
        }
        self.finish_questionnaire(cx);
    }

    fn finish_questionnaire(&mut self, cx: &mut Context<Self>) {
        let Some(questionnaire) = self.questionnaire.active_questionnaire().cloned() else {
            return;
        };
        let answers = (0..questionnaire.questions.len() as usize)
            .map(|index| self.questionnaire.recorded.get(&index).cloned())
            .collect();
        self.send_answer(questionnaire.item, answers, cx);
    }

    fn dismiss_questionnaire(&mut self, cx: &mut Context<Self>) {
        if self.questionnaire.sending.is_some() {
            return;
        }
        let Some(questionnaire) = self.questionnaire.active_questionnaire().cloned() else {
            return;
        };
        self.clear_answer_text(cx);
        let answers = vec![None; questionnaire.questions.len() as usize];
        self.send_answer(questionnaire.item, answers, cx);
    }

    fn send_answer(
        &mut self,
        item: ItemId,
        answers: Vec<Option<QuestionAnswer>>,
        cx: &mut Context<Self>,
    ) {
        if self.questionnaire.sending.is_some() {
            return;
        }
        let Some(host) = self.host(cx) else {
            return;
        };
        self.questionnaire
            .payloads
            .insert(item.clone(), answers.clone());
        self.questionnaire.error = None;
        self.questionnaire.sending = Some(item.clone());
        self.sync_editor_enabled(cx);
        let client = host.read(cx).client.clone();
        let generation = self.target_generation;
        cx.spawn(async move |this, cx| {
            let result = client
                .answer(&item, &Answer::Question { answers })
                .await
                .map_err(|error| {
                    tracing::warn!(%item, "couldn't send the answer: {error:#}");
                    format!("Couldn't send your answer: {error:#}")
                });
            let _ = this.update(cx, |this, cx| {
                if this.target_generation == generation {
                    this.answer_settled(&item, result.err(), cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Forge's verdict on a sent answer: accepted keeps the questionnaire
    /// hidden until it settles; a failure shows why and can be answered
    /// again.
    fn answer_settled(&mut self, item: &ItemId, error: Option<String>, cx: &mut Context<Self>) {
        if self.questionnaire.sending.as_ref() != Some(item) {
            return;
        }
        self.questionnaire.sending = None;
        match error {
            None => {
                self.questionnaire.answered.push(item.clone());
                self.questionnaire.drafts.remove(item);
                self.questionnaire.payloads.remove(item);
                self.questionnaire.active = None;
                self.questionnaire.error = None;
                self.questionnaire.reset_progress();
                self.load_answer_progress(cx);
            }
            Some(error) => self.questionnaire.error = Some(error),
        }
        self.sync_answer_mode(cx);
        self.sync_editor_enabled(cx);
        cx.notify();
    }

    fn activate_questionnaire(&mut self, item: ItemId, cx: &mut Context<Self>) {
        if self.questionnaire.sending.is_some() || self.questionnaire.active.as_ref() == Some(&item)
        {
            return;
        }
        self.save_answer_progress(cx);
        self.questionnaire.source = None;
        self.questionnaire.active = Some(item);
        self.load_answer_progress(cx);
        self.sync_question_window(cx);
        cx.notify();
    }

    fn save_answer_progress(&mut self, cx: &Context<Self>) {
        if !self.writing_answer() {
            return;
        }
        let Some(item) = self
            .questionnaire
            .active_questionnaire()
            .map(|entry| entry.item.clone())
        else {
            return;
        };
        self.questionnaire.drafts.insert(
            item,
            AnswerDraft {
                step: self.questionnaire.step,
                recorded: self.questionnaire.recorded.clone(),
                ticked: self.questionnaire.ticked.clone(),
                text: self.editor.read(cx).text().to_owned(),
                error: self.questionnaire.error.clone(),
            },
        );
    }

    fn load_answer_progress(&mut self, cx: &mut Context<Self>) {
        let draft = self
            .questionnaire
            .active_questionnaire()
            .and_then(|entry| self.questionnaire.drafts.get(&entry.item))
            .cloned()
            .unwrap_or_default();
        self.questionnaire.step = draft.step.min(
            self.questionnaire
                .active_questionnaire()
                .map_or(0, |entry| {
                    (entry.questions.len() as usize).saturating_sub(1)
                }),
        );
        self.questionnaire.recorded = draft.recorded;
        self.questionnaire.ticked = draft.ticked;
        self.questionnaire.error = draft.error;
        if self.writing_answer() {
            self.editor
                .update(cx, |editor, cx| editor.set_text(&draft.text, cx));
            self.has_text = !draft.text.trim().is_empty();
        }
    }

    fn retry_answer(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self
            .questionnaire
            .active_questionnaire()
            .map(|entry| entry.item.clone())
        else {
            return;
        };
        if let Some(payload) = self.questionnaire.payloads.get(&item).cloned() {
            self.send_answer(item, payload, cx);
        }
    }

    fn toggle_suggested_answer(&mut self, id: String, cx: &mut Context<Self>) {
        let ticked = &mut self.questionnaire.ticked;
        if let Some(position) = ticked.iter().position(|known| known == &id) {
            ticked.remove(position);
        } else {
            ticked.push(id);
        }
        cx.notify();
    }

    /// The editor's placeholder while a questionnaire is open.
    pub(crate) fn answer_placeholder(&self, cx: &gpui::App) -> Option<&'static str> {
        if !self.writing_answer() {
            return None;
        }
        let Some(question) = self.questionnaire.current_question(cx) else {
            return Some("Loading question…");
        };
        Some(if question.options.is_empty() {
            "Type your answer, then press Enter"
        } else {
            "Pick an answer above, or type your own"
        })
    }

    /// Questions share the same accessible rows and controls as other pages.
    pub(crate) fn render_questionnaires(
        &mut self,
        desktop: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.sync_question_window(cx);
        let active_item = self.questionnaire.active_questionnaire()?.item.clone();
        let question_count = self.questionnaire.active_questionnaire()?.questions.len();
        let Some(question) = self.questionnaire.current_question(cx).cloned() else {
            let window = self.questionnaire.question_window.clone();
            let error = window
                .as_ref()
                .and_then(|window| match window.read(cx).state() {
                    editor_core::CollectionWindowState::Failed(error) => Some(error.clone()),
                    _ => None,
                });
            let mut loading = div().child(error.unwrap_or_else(|| "Loading question…".into()));
            if let Some(window) = window {
                loading = loading.child(
                    Button::text("question-page-retry", *cx.theme(), "Retry").on_activate(
                        move |_, _, cx| window.update(cx, |window, cx| window.retry(cx)),
                    ),
                );
            }
            return Some(loading.into_any_element());
        };
        self.sync_options_browser(&question, cx);
        let sending = self.questionnaire.sending.is_some();
        let theme = *cx.theme();
        let owner = cx.entity().downgrade();
        let dismiss = div()
            .id("composer-question-dismiss")
            .role(gpui::Role::Button)
            .aria_label("Dismiss questionnaire")
            .when(!sending, |row| row.focusable().tab_index(0))
            .size(px(20.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(RadiusTokens::value(RadiusStep::Sm))
            .cursor_pointer()
            .hover(move |style| {
                style
                    .bg(desktop.selected.to_paint())
                    .text_color(desktop.workspace_foreground.to_paint())
            })
            .debug_selector(|| "composer-question-dismiss".into())
            .child(asset_glyph(AssetId::TABLER_X).size(px(14.0)))
            .on_click(move |_, _, cx| {
                let _ = owner.update(cx, |this, cx| this.dismiss_questionnaire(cx));
            });
        let mut card =
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(4.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(TypographyTokens::size(TextStep::Xs))
                        .line_height(px(16.0))
                        .text_color(desktop.workspace_secondary.to_paint())
                        .child(asset_glyph(AssetId::TABLER_MESSAGE_CIRCLE).size(px(14.0)))
                        .child(
                            div().flex_1().min_w_0().truncate().child(
                                question.header.clone().unwrap_or_else(|| "Question".into()),
                            ),
                        )
                        .child(dismiss),
                )
                .child(
                    div()
                        .px(px(2.0))
                        .text_size(TypographyTokens::size(TextStep::SmMid))
                        .line_height(px(22.0))
                        .text_color(desktop.workspace_foreground.to_paint())
                        .whitespace_normal()
                        .child(match &question.text {
                            artisan_api::TextContent::Inline(text) => {
                                div().child(text.clone()).into_any_element()
                            }
                            artisan_api::TextContent::Stored(source) => {
                                let source = source.clone();
                                Button::text("question-source", theme, "Read full question")
                                    .on_activate(cx.listener(move |this, _, _, cx| {
                                        this.open_question_source(source.clone(), theme, cx)
                                    }))
                                    .into_any_element()
                            }
                        }),
                );
        let mut rows = div().flex().flex_col().gap(px(2.0));
        if let Some(options) = &self.questionnaire.options {
            rows = rows.child(options.clone());
        }
        if let Collection::Inline(options) = &question.options {
            for (index, option) in options.iter().enumerate() {
                let label = option.label.clone();
                let id = option.id.clone();
                let multi = question.multi_select;
                let selected = self.questionnaire.ticked.contains(&id);
                let expected_item = active_item.clone();
                let expected_step = self.questionnaire.step;
                let expected_question = question.id.clone();
                let owner = cx.entity().downgrade();
                let marker = if multi {
                    div()
                        .size(px(16.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(RadiusTokens::value(RadiusStep::Xs))
                        .border_1()
                        .border_color(desktop.field_line.to_paint())
                        .when(selected, |marker| {
                            marker.child(asset_glyph(AssetId::TABLER_CHECK).size(px(12.0)))
                        })
                } else {
                    div()
                        .w(px(16.0))
                        .flex_none()
                        .text_color(desktop.workspace_secondary.to_paint())
                        .child(format!("{}", index + 1))
                };
                let mut text = div().flex().flex_col().min_w_0().child(
                    div()
                        .text_color(desktop.workspace_foreground.to_paint())
                        .child(label.clone()),
                );
                if let Some(description) = &option.description {
                    text = text.child(
                        div()
                            .text_size(TypographyTokens::size(TextStep::Xs))
                            .line_height(px(16.0))
                            .text_color(desktop.workspace_secondary.to_paint())
                            .child(match description {
                                artisan_api::TextContent::Inline(text) => text.clone(),
                                artisan_api::TextContent::Stored(_) => {
                                    "Full description available below".into()
                                }
                            }),
                    );
                }
                let row = div()
                    .id(SharedString::from(format!(
                        "composer-question-option-{index}"
                    )))
                    .role(gpui::Role::Button)
                    .aria_label(label.clone())
                    .aria_selected(selected)
                    .when(!sending, |row| row.focusable().tab_index(0))
                    .flex()
                    .items_start()
                    .gap(px(10.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(RadiusTokens::value(RadiusStep::Lg))
                    .text_size(TypographyTokens::size(TextStep::Sm))
                    .line_height(px(20.0))
                    .when(selected, |row| row.bg(desktop.selected.to_paint()))
                    .when(!sending, |row| {
                        row.cursor_pointer()
                            .hover(move |style| style.bg(desktop.selected.to_paint()))
                    })
                    .child(marker)
                    .child(text)
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if this.questionnaire.sending.is_some()
                                || this.questionnaire.active.as_ref() != Some(&expected_item)
                                || this.questionnaire.step != expected_step
                                || this.questionnaire.current_question(cx).map(|q| &q.id)
                                    != Some(&expected_question)
                            {
                                return;
                            }
                            if multi {
                                this.toggle_suggested_answer(id.clone(), cx);
                            } else {
                                this.clear_answer_text(cx);
                                this.answer_current(
                                    QuestionAnswer::Choices {
                                        ids: vec![id.clone()].into(),
                                    },
                                    cx,
                                );
                            }
                        });
                    });
                let mut option_row = div()
                    .debug_selector(move || format!("composer-question-option-{index}"))
                    .child(row);
                if let Some(artisan_api::TextContent::Stored(source)) = &option.description {
                    let source = source.clone();
                    option_row = option_row.child(
                        Button::text(
                            format!("question-description-{index}"),
                            theme,
                            "Read description",
                        )
                        .on_activate(cx.listener(move |this, _, _, cx| {
                            this.open_question_source(source.clone(), theme, cx)
                        })),
                    );
                }
                rows = rows.child(option_row);
            }
        }
        if let Some(source) = &self.questionnaire.source {
            rows = rows.child(source.clone());
        }
        if question.multi_select && !self.questionnaire.ticked.is_empty() && !sending {
            let owner = cx.entity().downgrade();
            rows = rows.child(
                div()
                    .id("composer-question-continue")
                    .role(gpui::Role::Button)
                    .aria_label(if (self.questionnaire.step as u64) + 1 < question_count {
                        "Next"
                    } else {
                        "Send answers"
                    })
                    .self_end()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(RadiusTokens::value(RadiusStep::Md))
                    .text_size(TypographyTokens::size(TextStep::XsMid))
                    .bg(desktop.primary_action.to_paint())
                    .text_color(desktop.primary_action_foreground.to_paint())
                    .when(!sending, |button| {
                        button.focusable().tab_index(0).cursor_pointer()
                    })
                    .debug_selector(|| "composer-question-continue".into())
                    .child(if (self.questionnaire.step as u64) + 1 < question_count {
                        "Next"
                    } else {
                        "Send answers"
                    })
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if this.questionnaire.sending.is_some() {
                                return;
                            }
                            let ticked = std::mem::take(&mut this.questionnaire.ticked);
                            this.clear_answer_text(cx);
                            this.answer_current(QuestionAnswer::Choices { ids: ticked.into() }, cx);
                        });
                    }),
            );
        }
        card = card.child(rows);
        if sending {
            card = card.child(
                div()
                    .px(px(2.0))
                    .text_size(TypographyTokens::size(TextStep::Xs))
                    .text_color(desktop.workspace_secondary.to_paint())
                    .child("Sending your answer…"),
            );
        } else if let Some(error) = &self.questionnaire.error {
            let owner = cx.entity().downgrade();
            card = card
                .child(
                    div()
                        .text_color(theme.colors.destructive.to_paint())
                        .child(error.clone()),
                )
                .child(
                    Button::text("composer-question-retry", theme, "Retry answer").on_activate(
                        move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| this.retry_answer(cx));
                        },
                    ),
                );
        }
        let mut stack = div().flex().flex_col().gap(px(2.0));
        let mut stacked = false;
        for (index, other) in self
            .questionnaire
            .visible()
            .filter(|entry| entry.item != active_item)
            .enumerate()
        {
            let item = other.item.clone();
            let label = other
                .questions
                .inline()
                .and_then(|questions| questions.first())
                .map(|q| match &q.text {
                    artisan_api::TextContent::Inline(text) => text.clone(),
                    artisan_api::TextContent::Stored(_) => {
                        q.header.clone().unwrap_or_else(|| "Questionnaire".into())
                    }
                })
                .unwrap_or_else(|| "Questionnaire".into());
            let owner = cx.entity().downgrade();
            let questions = other.questions.len();
            stacked = true;
            stack = stack.child(
                div()
                    .id(SharedString::from(format!("questionnaire-{item}")))
                    .debug_selector(move || format!("composer-questionnaire-stacked-{index}"))
                    .role(gpui::Role::Button)
                    .aria_label(label.clone())
                    .when(!sending, |row| row.focusable().tab_index(0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(RadiusTokens::value(RadiusStep::Md))
                    .text_size(TypographyTokens::size(TextStep::XsMid))
                    .line_height(px(18.0))
                    .text_color(desktop.workspace_secondary.to_paint())
                    .when(!sending, |row| {
                        row.cursor_pointer().hover(move |style| {
                            style
                                .bg(desktop.selected.to_paint())
                                .text_color(desktop.workspace_foreground.to_paint())
                        })
                    })
                    .child(asset_glyph(AssetId::TABLER_CHEVRON_RIGHT).size(px(14.0)))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .when(questions > 1, |row| {
                        row.child(format!("{questions} questions"))
                    })
                    .on_click(move |_, _, cx| {
                        let _ = owner
                            .update(cx, |this, cx| this.activate_questionnaire(item.clone(), cx));
                    }),
            );
        }
        let mut panel = div()
            .id("composer-questionnaire")
            .debug_selector(|| "composer-questionnaire".to_owned())
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .pb(px(8.0))
            .mb(px(4.0))
            .border_b_1()
            .border_color(desktop.line.to_paint())
            .child(card);
        if stacked {
            panel = panel.child(stack);
        }
        Some(panel.into_any_element())
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::{
        ComposerTarget, Source,
        tests::{Fixture, wait},
    };
    use gpui::TestAppContext;
    fn questionnaire() -> ComposerQuestionnaire {
        ComposerQuestionnaire {
            item: ItemId::new(),
            questions: vec![Question {
                id: "choice".into(),
                header: None,
                text: "Choose".into(),
                multi_select: false,
                options: Vec::new().into(),
            }]
            .into(),
        }
    }
    #[gpui::test]
    fn typed_answers_survive_switching_and_failure_does_not_erase_payload(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        let target = ComposerTarget::NewThread {
            host: fixture.host.clone(),
            project: Some(fixture.project.clone()),
        };
        let (composer, cx) = fixture.open(cx, target);
        let first = questionnaire();
        let second = questionnaire();
        composer.update(cx, |this, cx| {
            this.editor
                .update(cx, |editor, cx| editor.set_text("original draft", cx));
            this.has_text = true;
            this.set_questionnaires(vec![first.clone(), second.clone()], cx);
            assert_eq!(this.questionnaire.aside_text(), Some("original draft"));
            this.editor
                .update(cx, |editor, cx| editor.set_text("first answer", cx));
            this.has_text = true;
            this.activate_questionnaire(second.item.clone(), cx);
            this.editor
                .update(cx, |editor, cx| editor.set_text("second answer", cx));
            this.has_text = true;
            this.activate_questionnaire(first.item.clone(), cx);
            assert_eq!(this.editor.read(cx).text(), "first answer");
            this.submit_written_answer(cx);
            assert!(this.editor.read(cx).is_disabled());
            this.editor
                .update(cx, |editor, cx| editor.replace(0..0, "late answer", cx));
            assert!(this.editor.read(cx).text().is_empty());
            let payload = this.questionnaire.payloads[&first.item].clone();
            this.dismiss_questionnaire(cx);
            assert_eq!(
                this.questionnaire.payloads[&first.item], payload,
                "dismiss cannot replace an in-flight answer"
            );
        });
        wait(cx, "the missing question response", |cx| {
            cx.read(|cx| composer.read(cx).questionnaire.error.is_some())
        });
        composer.update(cx, |this, cx| {
            assert!(!this.editor.read(cx).is_disabled());
            assert_eq!(
                this.questionnaire.payloads[&first.item],
                vec![Some(QuestionAnswer::Text {
                    text: "first answer".into()
                })]
            );
            this.activate_questionnaire(second.item.clone(), cx);
            assert_eq!(this.editor.read(cx).text(), "second answer");
            this.activate_questionnaire(first.item.clone(), cx);
            this.retry_answer(cx);
            assert_eq!(
                this.questionnaire.payloads[&first.item],
                vec![Some(QuestionAnswer::Text {
                    text: "first answer".into()
                })]
            );
        });
    }
    #[test]
    fn large_answer_drafts_replace_by_ordinal_and_keep_skipped_positions() {
        let mut draft = AnswerDraft::default();
        for ordinal in (0..5000).step_by(2) {
            draft.recorded.insert(
                ordinal,
                QuestionAnswer::Text {
                    text: ordinal.to_string(),
                },
            );
        }
        draft.recorded.insert(
            4998,
            QuestionAnswer::Text {
                text: "changed".into(),
            },
        );
        let submitted: Vec<_> = (0..5000)
            .map(|ordinal| draft.recorded.get(&ordinal).cloned())
            .collect();
        assert_eq!(draft.recorded.len(), 2500);
        assert!(submitted[4999].is_none());
        assert_eq!(
            submitted[4998],
            Some(QuestionAnswer::Text {
                text: "changed".into()
            })
        );
    }

    #[gpui::test]
    fn duplicate_option_labels_submit_the_clicked_opaque_id(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        let (composer, cx) = fixture.open(
            cx,
            ComposerTarget::NewThread {
                host: fixture.host.clone(),
                project: Some(fixture.project.clone()),
            },
        );
        let mut question = questionnaire();
        question.questions.inline_mut().unwrap()[0].options = vec![
            artisan_api::QuestionOption {
                id: "first-id".into(),
                label: "Same label".into(),
                description: None,
            },
            artisan_api::QuestionOption {
                id: "second-id".into(),
                label: "Same label".into(),
                description: None,
            },
        ]
        .into();
        composer.update(cx, |this, cx| {
            this.set_questionnaires(vec![question.clone()], cx)
        });
        cx.run_until_parked();
        let second = cx
            .debug_bounds("composer-question-option-1")
            .expect("second choice rendered");
        cx.simulate_click(second.center(), gpui::Modifiers::none());
        composer.read_with(cx, |this, _| {
            assert_eq!(
                this.questionnaire.payloads[&question.item],
                vec![Some(QuestionAnswer::Choices {
                    ids: vec!["second-id".into()].into()
                })]
            );
        });
        wait(cx, "missing question response", |cx| {
            cx.read(|cx| composer.read(cx).questionnaire.error.is_some())
        });
        composer.update(cx, |this, cx| {
            this.retry_answer(cx);
            assert_eq!(
                this.questionnaire.payloads[&question.item],
                vec![Some(QuestionAnswer::Choices {
                    ids: vec!["second-id".into()].into()
                })]
            );
        });
    }

    #[gpui::test]
    fn typed_text_is_separate_from_selected_choice_ids(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        let (composer, cx) = fixture.open(
            cx,
            ComposerTarget::NewThread {
                host: fixture.host.clone(),
                project: Some(fixture.project.clone()),
            },
        );
        let mut question = questionnaire();
        question.questions.inline_mut().unwrap()[0].multi_select = true;
        question.questions.inline_mut().unwrap()[0].options = vec![artisan_api::QuestionOption {
            id: "choice-id".into(),
            label: "Choice".into(),
            description: None,
        }]
        .into();
        composer.update(cx, |this, cx| {
            this.set_questionnaires(vec![question.clone()], cx);
            this.toggle_suggested_answer("choice-id".into(), cx);
            this.editor
                .update(cx, |editor, cx| editor.set_text("Choice", cx));
            this.has_text = true;
            this.submit_written_answer(cx);
            assert_eq!(
                this.questionnaire.payloads[&question.item],
                vec![Some(QuestionAnswer::Text {
                    text: "Choice".into()
                })]
            );
        });
    }

    #[gpui::test]
    fn image_conversion_and_upload_continue_while_answering(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        let target = ComposerTarget::NewThread {
            host: fixture.host.clone(),
            project: Some(fixture.project.clone()),
        };
        let (composer, cx) = fixture.open(cx, target);
        let mut bytes = Vec::new();
        image::DynamicImage::new_rgba8(32, 32)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        composer.update(cx, |this, cx| {
            this.add_image(Source::Bytes(bytes), "test.png".into(), cx);
            this.set_questionnaires(vec![questionnaire()], cx);
            assert_eq!(this.attachments.len(), 1);
        });
        wait(cx, "attachment upload during questionnaire", |cx| {
            cx.read(|cx| {
                composer
                    .read(cx)
                    .attachments
                    .first()
                    .is_some_and(|attachment| attachment.state.hash().is_some())
            })
        });
        composer.update(cx, |this, cx| {
            assert!(this.writing_answer());
            this.set_questionnaires(Vec::new(), cx);
            assert_eq!(this.attachments.len(), 1);
            assert!(this.attachments[0].state.hash().is_some());
        });
    }
}

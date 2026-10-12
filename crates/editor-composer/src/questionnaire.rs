//! Agent questions answered from the composer card
//! (`native_composer/questionnaire.rs`).
//!
//! When an agent asks, the card extends upward with the open questionnaires
//! stacked above the editor: the one being answered in full, the others as
//! one-line rows beneath it that bring themselves forward when clicked. A
//! question shows its suggested answers as rows, and the editor below is
//! where an answer of your own is typed: while a questionnaire is open,
//! sending records the typed text as the current question's answer instead
//! of sending a message.
//!
//! Opening a questionnaire puts the draft aside (text and images) and
//! clears the editor for the answer. The aside draft stays Forge's draft
//! throughout: answer keystrokes are not draft changes, so nothing
//! overwrites it. It comes back once the last open questionnaire closes or
//! the composer moves to another thread.
//!
//! Questions stay open until answered. Dismissing a questionnaire skips all
//! its questions.

use crate::{Attachment, Composer};
use artisan_api::{Answer, ItemId, Question};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::theme::DesktopTheme;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};

/// One open questionnaire: the questions one agent request asked together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerQuestionnaire {
    /// The question item the answer goes to.
    pub item: ItemId,
    pub questions: Vec<Question>,
}

/// The draft put aside while an answer is written.
struct DraftAside {
    text: String,
    attachments: Vec<Attachment>,
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
    /// Answers recorded so far, by question index; empty skips.
    recorded: Vec<(usize, Vec<String>)>,
    /// Suggested answers ticked on the current multi-select question.
    ticked: Vec<String>,
    aside: Option<DraftAside>,
    /// The questionnaire whose answer is in flight.
    sending: Option<ItemId>,
    /// Why the last answer failed, shown until the next attempt.
    error: Option<String>,
}

impl QuestionnairePanel {
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

    fn current_question(&self) -> Option<&Question> {
        self.active_questionnaire()?.questions.get(self.step)
    }

    fn reset_progress(&mut self) {
        self.step = 0;
        self.recorded.clear();
        self.ticked.clear();
    }

    /// The draft text Forge keeps while an answer is written.
    pub(crate) fn aside_text(&self) -> Option<&str> {
        self.aside.as_ref().map(|aside| aside.text.as_str())
    }

    /// The draft images Forge keeps while an answer is written.
    pub(crate) fn aside_attachments(&self) -> Option<&[Attachment]> {
        self.aside
            .as_ref()
            .map(|aside| aside.attachments.as_slice())
    }
}

impl Composer {
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
            self.questionnaire.reset_progress();
            self.clear_answer_text(cx);
        }
        self.sync_answer_mode(cx);
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
        self.questionnaire.aside = Some(DraftAside {
            text,
            attachments: std::mem::take(&mut self.attachments),
        });
        self.editor.update(cx, |editor, cx| editor.set_text("", cx));
        self.has_text = false;
    }

    /// Brings the draft back, keeping any image added meanwhile.
    fn return_draft_aside(&mut self, cx: &mut Context<Self>) {
        let Some(aside) = self.questionnaire.aside.take() else {
            return;
        };
        let added = std::mem::take(&mut self.attachments);
        self.editor
            .update(cx, |editor, cx| editor.set_text(&aside.text, cx));
        self.has_text = !aside.text.trim().is_empty();
        self.attachments = aside.attachments;
        self.attachments.extend(added);
    }

    /// Records the typed text as the current question's answer, with any
    /// suggested answers ticked on a multi-select question.
    pub(crate) fn submit_written_answer(&mut self, cx: &mut Context<Self>) {
        let answer = self.editor.read(cx).text().trim().to_owned();
        if answer.is_empty() || self.questionnaire.sending.is_some() {
            return;
        }
        let mut answers = std::mem::take(&mut self.questionnaire.ticked);
        answers.push(answer);
        self.clear_answer_text(cx);
        self.answer_current(answers, cx);
    }

    fn answer_current(&mut self, answers: Vec<String>, cx: &mut Context<Self>) {
        if self.questionnaire.current_question().is_none() {
            return;
        }
        let step = self.questionnaire.step;
        self.questionnaire.error = None;
        self.questionnaire
            .recorded
            .retain(|(index, _)| *index != step);
        self.questionnaire.recorded.push((step, answers));
        self.questionnaire.ticked.clear();
        let count = self
            .questionnaire
            .active_questionnaire()
            .map_or(0, |questionnaire| questionnaire.questions.len());
        if step + 1 < count {
            self.questionnaire.step += 1;
            cx.notify();
            return;
        }
        self.finish_questionnaire(cx);
    }

    fn finish_questionnaire(&mut self, cx: &mut Context<Self>) {
        let Some(questionnaire) = self.questionnaire.active_questionnaire().cloned() else {
            return;
        };
        let answers = (0..questionnaire.questions.len())
            .map(|index| {
                self.questionnaire
                    .recorded
                    .iter()
                    .find(|(question, _)| *question == index)
                    .map(|(_, answers)| answers.clone())
                    .filter(|answers| !answers.is_empty())
            })
            .collect();
        self.send_answer(questionnaire.item, answers, cx);
    }

    fn dismiss_questionnaire(&mut self, cx: &mut Context<Self>) {
        let Some(questionnaire) = self.questionnaire.active_questionnaire().cloned() else {
            return;
        };
        self.clear_answer_text(cx);
        let answers = vec![None; questionnaire.questions.len()];
        self.send_answer(questionnaire.item, answers, cx);
    }

    fn send_answer(
        &mut self,
        item: ItemId,
        answers: Vec<Option<Vec<String>>>,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.host(cx) else {
            return;
        };
        self.questionnaire.sending = Some(item.clone());
        let client = host.read(cx).client.clone();
        cx.spawn(async move |this, cx| {
            let result = client
                .answer(&item, &Answer::Question { answers })
                .await
                .map_err(|error| {
                    tracing::warn!(%item, "couldn't send the answer: {error:#}");
                    format!("Couldn't send your answer: {error:#}")
                });
            let _ = this.update(cx, |this, cx| this.answer_settled(&item, result.err(), cx));
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
                self.questionnaire.active = None;
                self.questionnaire.error = None;
            }
            Some(error) => self.questionnaire.error = Some(error),
        }
        self.questionnaire.reset_progress();
        self.sync_answer_mode(cx);
        cx.notify();
    }

    fn activate_questionnaire(&mut self, item: ItemId, cx: &mut Context<Self>) {
        if self.questionnaire.sending.is_some() || self.questionnaire.active.as_ref() == Some(&item)
        {
            return;
        }
        self.questionnaire.active = Some(item);
        self.questionnaire.reset_progress();
        self.questionnaire.error = None;
        self.clear_answer_text(cx);
        cx.notify();
    }

    fn toggle_suggested_answer(&mut self, label: String, cx: &mut Context<Self>) {
        let ticked = &mut self.questionnaire.ticked;
        if let Some(position) = ticked.iter().position(|known| known == &label) {
            ticked.remove(position);
        } else {
            ticked.push(label);
        }
        cx.notify();
    }

    /// The editor's placeholder while a questionnaire is open.
    pub(crate) fn answer_placeholder(&self) -> Option<&'static str> {
        if !self.writing_answer() {
            return None;
        }
        let question = self.questionnaire.current_question()?;
        Some(if question.options.is_empty() {
            "Type your answer, then press Enter"
        } else {
            "Pick an answer above, or type your own"
        })
    }

    /// The stacked questionnaires above the editor, or nothing when no
    /// question is open.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder lays out the stacked questionnaires, the active question and its answer rows in visual order"
    )]
    pub(crate) fn render_questionnaires(
        &self,
        desktop: DesktopTheme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let active = self.questionnaire.active_questionnaire()?;
        let question = active.questions.get(self.questionnaire.step)?;
        let sending = self.questionnaire.sending.is_some();
        let muted = desktop.secondary;
        let foreground = desktop.foreground;
        let hover = desktop.selected;
        let count = active.questions.len();

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .text_size(px(12.0))
            .line_height(px(16.0))
            .text_color(muted)
            .child(asset_glyph(AssetId::TABLER_MESSAGE_CIRCLE).size(px(14.0)))
            .child(
                div().flex_1().min_w(px(0.0)).truncate().child(
                    question
                        .header
                        .clone()
                        .unwrap_or_else(|| "Question".to_owned()),
                ),
            )
            .child(
                div()
                    .id("composer-question-dismiss")
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover).text_color(foreground))
                    .debug_selector(|| "composer-question-dismiss".to_owned())
                    .child(asset_glyph(AssetId::TABLER_X).size(px(14.0)))
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss_questionnaire(cx))),
            );

        let mut rows = div().flex().flex_col().gap(px(2.0));
        for (index, option) in question.options.iter().enumerate() {
            let ticked = self.questionnaire.ticked.contains(&option.label);
            let label = option.label.clone();
            let multi = question.multi_select;
            let mut text = div()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .child(div().text_color(foreground).child(option.label.clone()));
            if let Some(description) = &option.description {
                text = text.child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(muted)
                        .child(description.clone()),
                );
            }
            let marker = if multi {
                div()
                    .size(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(desktop.field_line)
                    .when(ticked, |marker| {
                        marker.child(asset_glyph(AssetId::TABLER_CHECK).size(px(12.0)))
                    })
            } else {
                div()
                    .w(px(16.0))
                    .flex_none()
                    .text_color(muted)
                    .child(format!("{}", index + 1))
            };
            rows = rows.child(
                div()
                    .id(SharedString::from(format!(
                        "composer-question-option-{index}"
                    )))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(10.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(10.0))
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .when(ticked, |row| row.bg(hover))
                    .when(!sending, |row| {
                        row.cursor_pointer().hover(move |style| style.bg(hover))
                    })
                    .debug_selector(move || format!("composer-question-option-{index}"))
                    .child(marker)
                    .child(text)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.questionnaire.sending.is_some() {
                            return;
                        }
                        if multi {
                            this.toggle_suggested_answer(label.clone(), cx);
                        } else {
                            this.clear_answer_text(cx);
                            this.answer_current(vec![label.clone()], cx);
                        }
                    })),
            );
        }
        if question.multi_select && !self.questionnaire.ticked.is_empty() && !sending {
            rows = rows.child(
                div()
                    .id("composer-question-continue")
                    .self_end()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .text_size(px(13.0))
                    .bg(desktop.primary_action)
                    .text_color(desktop.primary_action_foreground)
                    .cursor_pointer()
                    .debug_selector(|| "composer-question-continue".to_owned())
                    .child(if self.questionnaire.step + 1 < count {
                        "Next"
                    } else {
                        "Send answers"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        let ticked = std::mem::take(&mut this.questionnaire.ticked);
                        this.clear_answer_text(cx);
                        this.answer_current(ticked, cx);
                    })),
            );
        }

        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(4.0))
            .child(header)
            .child(
                div()
                    .px(px(2.0))
                    .text_size(px(15.0))
                    .line_height(px(22.0))
                    .text_color(foreground)
                    .whitespace_normal()
                    .child(question.text.clone()),
            )
            .child(rows);
        if sending {
            card = card.child(
                div()
                    .px(px(2.0))
                    .text_size(px(12.0))
                    .text_color(muted)
                    .child("Sending your answer…"),
            );
        } else if let Some(error) = &self.questionnaire.error {
            card = card.child(
                div()
                    .px(px(2.0))
                    .text_size(px(12.0))
                    .text_color(muted)
                    .child(error.clone()),
            );
        }

        // The other open questionnaires stack beneath as one-line rows.
        let mut stack = div().flex().flex_col().gap(px(2.0));
        let mut stacked = 0usize;
        for (index, other) in self
            .questionnaire
            .visible()
            .filter(|other| other.item != active.item)
            .enumerate()
        {
            stacked += 1;
            let Some(first) = other.questions.first() else {
                continue;
            };
            let item = other.item.clone();
            let questions = other.questions.len();
            stack = stack.child(
                div()
                    .id(SharedString::from(format!(
                        "composer-questionnaire-stacked-{index}"
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .text_color(muted)
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover).text_color(foreground))
                    .debug_selector(move || format!("composer-questionnaire-stacked-{index}"))
                    .child(asset_glyph(AssetId::TABLER_CHEVRON_RIGHT).size(px(14.0)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .child(first.text.clone()),
                    )
                    .when(questions > 1, |row| {
                        row.child(format!("{questions} questions"))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_questionnaire(item.clone(), cx);
                    })),
            );
        }

        let mut panel = div()
            .id("composer-questionnaire")
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .pb(px(8.0))
            .mb(px(4.0))
            .border_b_1()
            .border_color(desktop.line)
            .debug_selector(|| "composer-questionnaire".to_owned())
            .child(card);
        if stacked > 0 {
            panel = panel.child(stack);
        }
        Some(panel.into_any_element())
    }
}

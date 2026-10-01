//! Agent questionnaires answered from the composer card.
//!
//! When an agent asks questions (any engine; see `QuestionObservation`), the
//! composer card extends upward with the open questionnaires stacked above
//! the editor: the one being answered in full, the others as one-line rows
//! beneath it that bring themselves forward when clicked. A question shows
//! its suggested answers as rows, and the editor below is where an answer of
//! your own is typed: while a questionnaire is open, sending records what
//! was typed as the current question's answer instead of sending a message.
//!
//! Opening a questionnaire puts the draft that was in the composer aside
//! (text, images, caret) and clears the editor for the answer. The aside
//! draft stays the Forge's draft throughout: the draft sync keeps seeing it
//! and answer keystrokes are not draft changes, so nothing overwrites it. It
//! comes back as soon as the last open questionnaire closes or the thread
//! changes.
//!
//! Questions stay open until answered: the panel never times out and never
//! closes a questionnaire on its own. Dismissing a questionnaire resolves
//! all its questions with empty answers; the harness translates dismissal.

#![forbid(unsafe_code)]

use gpui::prelude::FluentBuilder as _;

use super::*;
use crate::composer_draft_sync::DraftBody;

/// Stable selector for the questionnaire panel.
pub(crate) const NATIVE_COMPOSER_QUESTIONNAIRE_SELECTOR: &str =
    "artisan-native-composer-questionnaire";

/// One open question as the composer presents it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComposerQuestion {
    /// Provider question identity the answer names.
    pub(crate) question_id: String,
    /// The question itself.
    pub(crate) text: String,
    /// Short category label, when the agent gave one.
    pub(crate) header: Option<String>,
    /// Whether several suggested answers may be chosen together.
    pub(crate) multi_select: bool,
    /// Suggested answers as `(label, description)`, recommended first.
    pub(crate) options: Vec<(String, Option<String>)>,
}

/// One open questionnaire: the questions one agent request asked together.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComposerQuestionnaire {
    /// Questionnaire identity the answer names.
    pub(crate) group_id: String,
    /// Its open questions, in order.
    pub(crate) questions: Vec<ComposerQuestion>,
}

/// A finished questionnaire answer for the application to send.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComposerQuestionnaireAnswer {
    /// The answered questionnaire.
    pub(crate) group_id: String,
    /// `(question_id, answers)` per question in order; an empty list skips.
    pub(crate) answers: Vec<(String, Vec<String>)>,
}

/// The draft put aside while an answer is written in the composer.
struct DraftAside {
    text: String,
    authored_text_present: bool,
    attachments: Vec<ComposerAttachment>,
    selection: Range<usize>,
}

/// The composer's questionnaire state.
#[derive(Default)]
pub(super) struct QuestionnairePanel {
    /// Open questionnaires, oldest first, as the application last pushed.
    questionnaires: Vec<ComposerQuestionnaire>,
    /// Questionnaires answered here and settled by the Forge, hidden until
    /// their resolution arrives and removes them from the pushed list.
    answered: Vec<String>,
    /// The questionnaire being answered; the oldest when unset.
    active: Option<String>,
    /// Index of the question being answered in the active questionnaire.
    step: usize,
    /// Answers recorded so far for the active questionnaire.
    recorded: Vec<(String, Vec<String>)>,
    /// Suggested answers ticked on the current multi-select question.
    ticked: Vec<String>,
    /// The draft put aside while a questionnaire is open.
    aside: Option<DraftAside>,
    /// The questionnaire whose answer is in flight.
    sending: Option<String>,
    /// Why the last answer failed, shown until the next attempt.
    error: Option<String>,
    /// The finished answer waiting for the application to take it.
    outbox: Option<ComposerQuestionnaireAnswer>,
}

impl QuestionnairePanel {
    fn visible(&self) -> impl Iterator<Item = &ComposerQuestionnaire> {
        self.questionnaires
            .iter()
            .filter(|questionnaire| !self.answered.contains(&questionnaire.group_id))
    }

    fn active_questionnaire(&self) -> Option<&ComposerQuestionnaire> {
        let mut visible = self.visible();
        match &self.active {
            Some(active) => self
                .visible()
                .find(|questionnaire| &questionnaire.group_id == active)
                .or_else(|| visible.next()),
            None => visible.next(),
        }
    }

    fn current_question(&self) -> Option<&ComposerQuestion> {
        self.active_questionnaire()?.questions.get(self.step)
    }

    fn reset_progress(&mut self) {
        self.step = 0;
        self.recorded.clear();
        self.ticked.clear();
    }
}

impl NativeComposer {
    /// Replaces the open questionnaires. Returns whether anything changed.
    ///
    /// A questionnaire that is no longer open (answered anywhere, or on
    /// another thread now) drops its progress. The draft goes aside while
    /// any questionnaire is open and comes back once none is.
    pub(crate) fn set_questionnaires(
        &mut self,
        questionnaires: Vec<ComposerQuestionnaire>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.questionnaire.questionnaires == questionnaires {
            return false;
        }
        let active_before = self
            .questionnaire
            .active_questionnaire()
            .map(|questionnaire| questionnaire.group_id.clone());
        let open = |group: &String| {
            questionnaires
                .iter()
                .any(|questionnaire| &questionnaire.group_id == group)
        };
        self.questionnaire.answered.retain(|group| open(group));
        if self
            .questionnaire
            .sending
            .as_ref()
            .is_some_and(|group| !open(group))
        {
            self.questionnaire.sending = None;
        }
        self.questionnaire.questionnaires = questionnaires;
        let active_after = self
            .questionnaire
            .active_questionnaire()
            .map(|questionnaire| questionnaire.group_id.clone());
        if active_before != active_after {
            self.questionnaire.active = active_after;
            self.questionnaire.reset_progress();
            self.clear_answer_text();
        }
        self.sync_answer_mode();
        cx.notify();
        true
    }

    /// Whether the editor is the answer field: a questionnaire is open, so
    /// sending records the typed text as the current question's answer
    /// instead of sending a message.
    pub(crate) fn writing_answer(&self) -> bool {
        self.questionnaire.aside.is_some()
    }

    /// Puts the draft aside while a questionnaire is open and brings it back
    /// once none is.
    fn sync_answer_mode(&mut self) {
        let open = self.questionnaire.active_questionnaire().is_some();
        match (open, self.questionnaire.aside.is_some()) {
            (true, false) => self.put_draft_aside(),
            (false, true) => self.return_draft_aside(),
            _ => {}
        }
    }

    /// Clears a partly typed answer; the draft aside is untouched.
    fn clear_answer_text(&mut self) {
        if self.writing_answer() && !self.state.draft().is_empty() {
            self.replace_draft_text(String::new());
        }
    }

    /// Takes the finished answer the application must send, once.
    pub(crate) fn take_questionnaire_answer(&mut self) -> Option<ComposerQuestionnaireAnswer> {
        self.questionnaire.outbox.take()
    }

    /// Records the Forge's verdict on a sent answer. A settled answer keeps
    /// its questionnaire hidden until the resolution arrives; a failed one
    /// shows why and can be answered again.
    pub(crate) fn questionnaire_settled(
        &mut self,
        group_id: &str,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.questionnaire.sending.as_deref() != Some(group_id) {
            return;
        }
        self.questionnaire.sending = None;
        match error {
            None => {
                self.questionnaire.answered.push(group_id.to_owned());
                self.questionnaire.active = None;
                self.questionnaire.error = None;
            }
            Some(error) => self.questionnaire.error = Some(error),
        }
        self.questionnaire.reset_progress();
        self.sync_answer_mode();
        cx.notify();
    }

    /// Records the typed text as the current question's answer, together
    /// with any suggested answers ticked on a multi-select question.
    pub(crate) fn submit_written_answer(&mut self, cx: &mut Context<Self>) {
        let answer = self.state.draft().trim().to_owned();
        if answer.is_empty() || self.questionnaire.sending.is_some() {
            return;
        }
        let mut answers = std::mem::take(&mut self.questionnaire.ticked);
        answers.push(answer);
        self.clear_answer_text();
        self.answer_current(answers, cx);
    }

    /// The draft the Forge stores: the one put aside while an answer is
    /// being written, so the answer never replaces it.
    pub(super) fn draft_aside_body(&self) -> Option<DraftBody> {
        let aside = self.questionnaire.aside.as_ref()?;
        Some(DraftBody {
            text: aside.text.clone(),
            attachments: aside
                .attachments
                .iter()
                .filter_map(|attachment| attachment.stored.clone())
                .collect(),
        })
    }

    /// Puts the draft aside and clears the editor for answers.
    pub(super) fn put_draft_aside(&mut self) {
        if self.questionnaire.aside.is_some() {
            return;
        }
        let aside = DraftAside {
            text: self.state.draft().to_owned(),
            authored_text_present: self.authored_text_present,
            attachments: std::mem::take(&mut self.attachments),
            selection: self.selection.clone(),
        };
        self.questionnaire.aside = Some(aside);
        self.replace_draft_text(String::new());
    }

    /// Brings the draft put aside back into the editor, keeping any image
    /// added while the answer was written.
    pub(super) fn return_draft_aside(&mut self) {
        let Some(aside) = self.questionnaire.aside.take() else {
            return;
        };
        let added = std::mem::take(&mut self.attachments);
        self.replace_draft_text(aside.text);
        self.authored_text_present = aside.authored_text_present;
        let end = self.state.draft().len();
        self.selection = aside.selection.start.min(end)..aside.selection.end.min(end);
        self.attachments = aside.attachments;
        self.attachments.extend(added);
    }

    fn answer_current(&mut self, answers: Vec<String>, cx: &mut Context<Self>) {
        let Some(question) = self.questionnaire.current_question().cloned() else {
            return;
        };
        self.questionnaire.error = None;
        self.questionnaire
            .recorded
            .retain(|(question_id, _)| question_id != &question.question_id);
        self.questionnaire
            .recorded
            .push((question.question_id, answers));
        self.questionnaire.ticked.clear();
        let count = self
            .questionnaire
            .active_questionnaire()
            .map_or(0, |questionnaire| questionnaire.questions.len());
        if self.questionnaire.step + 1 < count {
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
        let answers = questionnaire
            .questions
            .iter()
            .map(|question| {
                let answers = self
                    .questionnaire
                    .recorded
                    .iter()
                    .find(|(question_id, _)| question_id == &question.question_id)
                    .map(|(_, answers)| answers.clone())
                    .unwrap_or_default();
                (question.question_id.clone(), answers)
            })
            .collect();
        self.send_questionnaire_answer(questionnaire.group_id, answers, cx);
    }

    fn dismiss_questionnaire(&mut self, cx: &mut Context<Self>) {
        let Some(questionnaire) = self.questionnaire.active_questionnaire().cloned() else {
            return;
        };
        self.clear_answer_text();
        let answers = questionnaire
            .questions
            .iter()
            .map(|question| (question.question_id.clone(), Vec::new()))
            .collect();
        self.send_questionnaire_answer(questionnaire.group_id, answers, cx);
    }

    fn send_questionnaire_answer(
        &mut self,
        group_id: String,
        answers: Vec<(String, Vec<String>)>,
        cx: &mut Context<Self>,
    ) {
        self.questionnaire.sending = Some(group_id.clone());
        self.questionnaire.outbox = Some(ComposerQuestionnaireAnswer { group_id, answers });
        cx.emit(NativeComposerEvent::QuestionnaireAnswered);
        cx.notify();
    }

    fn activate_questionnaire(&mut self, group_id: String, cx: &mut Context<Self>) {
        if self.questionnaire.sending.is_some()
            || self.questionnaire.active.as_ref() == Some(&group_id)
        {
            return;
        }
        self.questionnaire.active = Some(group_id);
        self.questionnaire.reset_progress();
        self.questionnaire.error = None;
        self.clear_answer_text();
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

    /// The editor placeholder while a questionnaire is open.
    pub(super) fn answer_placeholder(&self) -> Option<&'static str> {
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

    /// Renders the stacked questionnaires above the editor, or nothing when
    /// no question is open.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder lays out the stacked questionnaires, the active question, and its answer rows in visual order"
    )]
    pub(super) fn render_questionnaires(
        &self,
        entity: &Entity<Self>,
        desktop_theme: DesktopTheme,
    ) -> Option<AnyElement> {
        let active = self.questionnaire.active_questionnaire()?;
        let question = active.questions.get(self.questionnaire.step)?;
        let sending = self.questionnaire.sending.is_some();
        let muted = desktop_theme.secondary;
        let foreground = desktop_theme.foreground;
        let hover = desktop_theme.selected;
        let count = active.questions.len();

        let mut header = div()
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
            );
        if count > 1 {
            header = header.child(format!("{} of {count}", self.questionnaire.step + 1));
        }
        let dismiss_entity = entity.clone();
        header = header.child(
            div()
                .id("artisan-native-composer-question-dismiss")
                .size(px(20.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(move |style| style.bg(hover).text_color(foreground))
                .aria_label("Dismiss these questions")
                .debug_selector(|| "artisan-native-composer-question-dismiss".to_owned())
                .child(asset_glyph(AssetId::TABLER_X).size(px(14.0)))
                .on_click(move |_, _, cx| {
                    dismiss_entity.update(cx, Self::dismiss_questionnaire);
                }),
        );

        let mut rows = div().flex().flex_col().gap(px(2.0));
        for (index, (label, description)) in question.options.iter().enumerate() {
            let ticked = self.questionnaire.ticked.contains(label);
            let row_entity = entity.clone();
            let row_label = label.clone();
            let multi = question.multi_select;
            let mut text = div()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .child(div().text_color(foreground).child(label.clone()));
            if let Some(description) = description {
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
                    .border_color(desktop_theme.field_line)
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
                    .id(ElementId::Name(
                        format!("artisan-native-composer-question-option-{index}").into(),
                    ))
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
                    .role(gpui::Role::Button)
                    .aria_label(label.clone())
                    .debug_selector(move || {
                        format!("artisan-native-composer-question-option-{index}")
                    })
                    .child(marker)
                    .child(text)
                    .on_click(move |_, _, cx| {
                        let label = row_label.clone();
                        row_entity.update(cx, |composer, cx| {
                            if composer.questionnaire.sending.is_some() {
                                return;
                            }
                            if multi {
                                composer.toggle_suggested_answer(label, cx);
                            } else {
                                composer.clear_answer_text();
                                composer.answer_current(vec![label], cx);
                            }
                        });
                    }),
            );
        }
        if question.multi_select && !self.questionnaire.ticked.is_empty() && !sending {
            let continue_entity = entity.clone();
            rows = rows.child(
                div()
                    .id("artisan-native-composer-question-continue")
                    .self_end()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .text_size(px(13.0))
                    .bg(desktop_theme.primary_action)
                    .text_color(desktop_theme.primary_action_foreground)
                    .cursor_pointer()
                    .debug_selector(|| "artisan-native-composer-question-continue".to_owned())
                    .child(if self.questionnaire.step + 1 < count {
                        "Next"
                    } else {
                        "Send answers"
                    })
                    .on_click(move |_, _, cx| {
                        continue_entity.update(cx, |composer, cx| {
                            let ticked = std::mem::take(&mut composer.questionnaire.ticked);
                            composer.clear_answer_text();
                            composer.answer_current(ticked, cx);
                        });
                    }),
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
            card = card.child(crate::dismissible_notice::DismissibleNotice::new(
                format!("questionnaire-{}-error-{error}", active.group_id),
                div()
                    .px(px(2.0))
                    .text_size(px(12.0))
                    .text_color(muted)
                    .child(error.clone()),
                ArtisanTheme::for_mode(ThemeMode::Dark),
            ));
        }

        // The other open questionnaires stack beneath as one-line rows.
        let mut stack = div().flex().flex_col().gap(px(2.0));
        let mut stacked = 0usize;
        for (index, other) in self
            .questionnaire
            .visible()
            .filter(|other| other.group_id != active.group_id)
            .enumerate()
        {
            stacked += 1;
            let Some(first) = other.questions.first() else {
                continue;
            };
            let group = other.group_id.clone();
            let stack_entity = entity.clone();
            let questions = other.questions.len();
            stack = stack.child(
                div()
                    .id(ElementId::Name(
                        format!("artisan-native-composer-questionnaire-stacked-{index}").into(),
                    ))
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
                    .debug_selector(move || {
                        format!("artisan-native-composer-questionnaire-stacked-{index}")
                    })
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
                    .on_click(move |_, _, cx| {
                        let group = group.clone();
                        stack_entity.update(cx, |composer, cx| {
                            composer.activate_questionnaire(group, cx);
                        });
                    }),
            );
        }

        let mut panel = div()
            .id(NATIVE_COMPOSER_QUESTIONNAIRE_SELECTOR)
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .pb(px(8.0))
            .mb(px(4.0))
            .border_b_1()
            .border_color(desktop_theme.line)
            .debug_selector(|| NATIVE_COMPOSER_QUESTIONNAIRE_SELECTOR.to_owned())
            .child(card);
        if stacked > 0 {
            panel = panel.child(stack);
        }
        Some(panel.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{Entity, TestAppContext, VisualTestContext, px, size};

    use super::super::{NativeComposer, NativeComposerEvent};
    use super::{ComposerQuestion, ComposerQuestionnaire, NATIVE_COMPOSER_QUESTIONNAIRE_SELECTOR};

    fn question(id: &str, options: &[&str], multi_select: bool) -> ComposerQuestion {
        ComposerQuestion {
            question_id: id.to_owned(),
            text: format!("Question {id}?"),
            header: None,
            multi_select,
            options: options
                .iter()
                .map(|label| ((*label).to_owned(), None))
                .collect(),
        }
    }

    fn questionnaire(group: &str, questions: Vec<ComposerQuestion>) -> ComposerQuestionnaire {
        ComposerQuestionnaire {
            group_id: group.to_owned(),
            questions,
        }
    }

    fn mount(cx: &mut TestAppContext) -> (Entity<NativeComposer>, &mut VisualTestContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        cx.simulate_resize(size(px(640.0), px(480.0)));
        (view, cx)
    }

    fn answered_events(
        view: &Entity<NativeComposer>,
        cx: &mut VisualTestContext,
    ) -> Rc<RefCell<usize>> {
        let answered = Rc::new(RefCell::new(0));
        let counter = answered.clone();
        cx.update(|_, app| {
            app.subscribe(view, move |_, event: &NativeComposerEvent, _| {
                if *event == NativeComposerEvent::QuestionnaireAnswered {
                    *counter.borrow_mut() += 1;
                }
            })
            .detach();
        });
        answered
    }

    #[gpui::test]
    fn an_open_questionnaire_makes_the_editor_the_answer_field(cx: &mut TestAppContext) {
        let (view, cx) = mount(cx);
        let answered = answered_events(&view, cx);
        cx.update(|_, app| {
            view.update(app, |composer, cx| {
                composer.set_draft("my half-written message");
                let change = composer.draft_change();
                composer.set_questionnaires(
                    vec![questionnaire(
                        "group-1",
                        vec![question("q1", &[], false), question("q2", &["Tests"], true)],
                    )],
                    cx,
                );
                assert!(composer.writing_answer());
                assert_eq!(
                    composer.state.draft(),
                    "",
                    "the editor clears for the answer"
                );
                assert_eq!(
                    composer.answer_placeholder(),
                    Some("Type your answer, then press Enter")
                );

                // Typing the answer is not a draft change, and the Forge keeps
                // seeing the draft that was put aside.
                composer.set_draft("Postgres, please");
                composer.note_draft_change();
                assert_eq!(composer.draft_change(), change);
                assert_eq!(composer.draft_body().text, "my half-written message");

                composer.submit_written_answer(cx);
                assert!(composer.writing_answer(), "the next question is still open");
                assert_eq!(composer.state.draft(), "", "the typed answer is recorded");
                assert_eq!(
                    composer.answer_placeholder(),
                    Some("Pick an answer above, or type your own")
                );

                // A typed answer joins the ticked suggestions on a multi-select.
                composer.toggle_suggested_answer("Tests".to_owned(), cx);
                composer.set_draft("and docs");
                composer.submit_written_answer(cx);
                let answer = composer
                    .take_questionnaire_answer()
                    .expect("the last question finishes the questionnaire");
                assert_eq!(answer.group_id, "group-1");
                assert_eq!(
                    answer.answers,
                    vec![
                        ("q1".to_owned(), vec!["Postgres, please".to_owned()]),
                        (
                            "q2".to_owned(),
                            vec!["Tests".to_owned(), "and docs".to_owned()]
                        ),
                    ]
                );

                // The draft stays aside while the answer is in flight and
                // comes back once the questionnaire settles.
                assert!(composer.writing_answer());
                composer.questionnaire_settled("group-1", None, cx);
                assert!(!composer.writing_answer());
                assert_eq!(composer.state.draft(), "my half-written message");
            });
        });
        cx.run_until_parked();
        assert_eq!(*answered.borrow(), 1);
    }

    #[gpui::test]
    fn closing_or_switching_questionnaires_keeps_the_draft(cx: &mut TestAppContext) {
        let (view, cx) = mount(cx);
        cx.update(|_, app| {
            view.update(app, |composer, cx| {
                composer.set_draft("keep me");
                composer.set_questionnaires(
                    vec![
                        questionnaire("group-1", vec![question("q1", &["Yes"], false)]),
                        questionnaire("group-2", vec![question("q2", &[], false)]),
                    ],
                    cx,
                );
                composer.set_draft("half an answer");
                // Bringing another questionnaire forward drops the half-typed
                // answer but keeps the draft aside.
                composer.activate_questionnaire("group-2".to_owned(), cx);
                assert!(composer.writing_answer());
                assert_eq!(composer.state.draft(), "");
                assert_eq!(composer.draft_body().text, "keep me");

                composer.set_draft("half an answer");
                // Every questionnaire closing elsewhere brings the draft back.
                composer.set_questionnaires(Vec::new(), cx);
                assert!(!composer.writing_answer());
                assert_eq!(composer.state.draft(), "keep me");
            });
        });
    }

    #[gpui::test]
    fn suggested_answers_skips_and_dismissals_answer_in_order(cx: &mut TestAppContext) {
        let (view, cx) = mount(cx);
        cx.update(|_, app| {
            view.update(app, |composer, cx| {
                composer.set_questionnaires(
                    vec![questionnaire(
                        "group-1",
                        vec![
                            question("q1", &["Postgres", "SQLite"], false),
                            question("q2", &["Tests", "Docs"], true),
                            question("q3", &[], false),
                        ],
                    )],
                    cx,
                );
                composer.answer_current(vec!["SQLite".to_owned()], cx);
                composer.toggle_suggested_answer("Tests".to_owned(), cx);
                composer.toggle_suggested_answer("Docs".to_owned(), cx);
                composer.toggle_suggested_answer("Tests".to_owned(), cx);
                let ticked = std::mem::take(&mut composer.questionnaire.ticked);
                composer.answer_current(ticked, cx);
                // Skipping the last question still sends the questionnaire.
                composer.answer_current(Vec::new(), cx);
                let answer = composer.take_questionnaire_answer().expect("answered");
                assert_eq!(
                    answer.answers,
                    vec![
                        ("q1".to_owned(), vec!["SQLite".to_owned()]),
                        ("q2".to_owned(), vec!["Docs".to_owned()]),
                        ("q3".to_owned(), Vec::new()),
                    ]
                );

                // A failed send reopens the questionnaire from its start.
                composer.questionnaire_settled("group-1", Some("offline".to_owned()), cx);
                composer.dismiss_questionnaire(cx);
                let dismissed = composer.take_questionnaire_answer().expect("dismissed");
                assert!(
                    dismissed
                        .answers
                        .iter()
                        .all(|(_, answers)| answers.is_empty())
                );
            });
        });
    }

    #[gpui::test]
    fn open_questionnaires_stack_and_a_settled_one_stays_hidden(cx: &mut TestAppContext) {
        let (view, cx) = mount(cx);
        cx.update(|_, app| {
            view.update(app, |composer, cx| {
                composer.set_questionnaires(
                    vec![
                        questionnaire("group-1", vec![question("q1", &["Yes"], false)]),
                        questionnaire("group-2", vec![question("q2", &["No"], false)]),
                    ],
                    cx,
                );
            });
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_QUESTIONNAIRE_SELECTOR)
                .is_some(),
            "open questions extend the card"
        );
        assert!(
            cx.debug_bounds("artisan-native-composer-questionnaire-stacked-0")
                .is_some(),
            "the other questionnaire stacks beneath the one being answered"
        );

        cx.update(|_, app| {
            view.update(app, |composer, cx| {
                composer.activate_questionnaire("group-2".to_owned(), cx);
                assert_eq!(
                    composer
                        .questionnaire
                        .active_questionnaire()
                        .map(|open| open.group_id.as_str()),
                    Some("group-2")
                );
                composer.answer_current(vec!["No".to_owned()], cx);
                let _ = composer.take_questionnaire_answer();
                composer.questionnaire_settled("group-2", None, cx);
                // Settled but not yet resolved: hidden, and the other one leads.
                assert_eq!(
                    composer
                        .questionnaire
                        .active_questionnaire()
                        .map(|open| open.group_id.as_str()),
                    Some("group-1")
                );
            });
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("artisan-native-composer-questionnaire-stacked-0")
                .is_none(),
            "a settled questionnaire no longer stacks"
        );

        cx.update(|_, app| {
            view.update(app, |composer, cx| {
                composer.set_questionnaires(Vec::new(), cx);
            });
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_QUESTIONNAIRE_SELECTOR)
                .is_none(),
            "the card returns to its plain height once every question is answered"
        );
    }
}

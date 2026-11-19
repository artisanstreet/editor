use artisan_api::{Answer, Collection, ItemId, Question, QuestionAnswer};
use editor_core::ForgeClient;
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerQuestionnaire {
    pub item: ItemId,
    pub questions: Collection<Question>,
}

#[derive(Clone, Debug, Default)]
struct AnswerDraft {
    step: usize,
    recorded: BTreeMap<usize, QuestionAnswer>,
    ticked: Vec<String>,
    text: String,
    error: Option<String>,
}

#[expect(
    clippy::partial_pub_fields,
    reason = "the workspace and tests read the active step and ticked options; per-question drafts stay private"
)]
#[derive(Debug, Default)]
pub struct QuestionnairePanel {
    questionnaires: Vec<ComposerQuestionnaire>,
    answered: Vec<ItemId>,
    active: Option<ItemId>,
    pub step: usize,
    recorded: BTreeMap<usize, QuestionAnswer>,
    pub ticked: Vec<String>,
    aside: Option<String>,
    pub sending: Option<ItemId>,
    pub(crate) error: Option<String>,
    drafts: HashMap<ItemId, AnswerDraft>,
    payloads: HashMap<ItemId, Vec<Option<QuestionAnswer>>>,
}

impl QuestionnairePanel {
    pub(crate) fn visible(&self) -> impl Iterator<Item = &ComposerQuestionnaire> {
        self.questionnaires
            .iter()
            .filter(|entry| !self.answered.contains(&entry.item))
    }

    pub fn active(&self) -> Option<&ComposerQuestionnaire> {
        self.active
            .as_ref()
            .and_then(|item| self.visible().find(|entry| &entry.item == item))
            .or_else(|| self.visible().next())
    }

    pub(crate) fn writing_answer(&self) -> bool {
        self.aside.is_some()
    }
    pub(crate) fn aside_text(&self) -> Option<&str> {
        self.aside.as_deref()
    }
    pub(crate) fn restore_aside_text(&mut self, text: &str) -> bool {
        match &mut self.aside {
            Some(aside) => {
                text.clone_into(aside);
                true
            }
            None => false,
        }
    }

    fn save(&mut self, text: &str) {
        if !self.writing_answer() {
            return;
        }
        let Some(item) = self.active().map(|entry| entry.item.clone()) else {
            return;
        };
        self.drafts.insert(
            item,
            AnswerDraft {
                step: self.step,
                recorded: self.recorded.clone(),
                ticked: self.ticked.clone(),
                text: text.to_owned(),
                error: self.error.clone(),
            },
        );
    }

    fn load(&mut self, text: &mut String) {
        let draft = self
            .active()
            .and_then(|entry| self.drafts.get(&entry.item))
            .cloned()
            .unwrap_or_default();
        let count = self.active().map_or(0, |entry| {
            usize::try_from(entry.questions.len()).unwrap_or(usize::MAX)
        });
        self.step = draft.step.min(count.saturating_sub(1));
        self.recorded = draft.recorded;
        self.ticked = draft.ticked;
        self.error = draft.error;
        if self.writing_answer() {
            *text = draft.text;
        }
    }

    fn sync_mode(&mut self, text: &mut String) {
        match (self.active().is_some(), self.aside.is_some()) {
            (true, false) => self.aside = Some(std::mem::take(text)),
            (false, true) => *text = self.aside.take().unwrap_or_default(),
            _ => {}
        }
    }

    pub fn holds<'a>(
        &self,
        questionnaires: impl ExactSizeIterator<Item = (&'a ItemId, &'a Collection<Question>)>,
    ) -> bool {
        self.questionnaires.len() == questionnaires.len()
            && self
                .questionnaires
                .iter()
                .zip(questionnaires)
                .all(|(held, (item, questions))| &held.item == item && &held.questions == questions)
    }

    pub fn set_questionnaires(
        &mut self,
        questionnaires: Vec<ComposerQuestionnaire>,
        text: &mut String,
    ) {
        if self.questionnaires == questionnaires {
            return;
        }
        self.save(text);
        let before = self.active().map(|entry| entry.item.clone());
        let replaced: Vec<ItemId> = questionnaires
            .iter()
            .filter(|entry| {
                self.questionnaires
                    .iter()
                    .any(|old| old.item == entry.item && old.questions != entry.questions)
            })
            .map(|entry| entry.item.clone())
            .collect();
        let open = |item: &ItemId| questionnaires.iter().any(|entry| &entry.item == item);
        let kept = |item: &ItemId| open(item) && !replaced.contains(item);
        self.answered.retain(&open);
        self.drafts.retain(|item, _| kept(item));
        self.payloads.retain(|item, _| kept(item));
        if before.as_ref().is_some_and(|item| replaced.contains(item)) {
            self.step = 0;
            self.recorded.clear();
            self.ticked.clear();
        }
        if self.sending.as_ref().is_some_and(|item| !open(item)) {
            self.sending = None;
        }
        self.questionnaires = questionnaires;
        let after = self.active().map(|entry| entry.item.clone());
        if before != after {
            self.active = after;
            self.load(text);
        }
        self.sync_mode(text);
    }

    pub fn activate(&mut self, item: ItemId, text: &mut String) {
        if self.sending.is_some() || self.active.as_ref() == Some(&item) {
            return;
        }
        self.save(text);
        self.active = Some(item);
        self.load(text);
    }

    pub(crate) fn toggle(&mut self, id: String) {
        if let Some(index) = self.ticked.iter().position(|known| known == &id) {
            self.ticked.remove(index);
        } else {
            self.ticked.push(id);
        }
    }

    pub fn answer(
        &mut self,
        answer: QuestionAnswer,
        question_loaded: bool,
        text: &mut String,
    ) -> Option<(ItemId, Answer)> {
        if !question_loaded || self.sending.is_some() {
            return None;
        }
        self.error = None;
        self.recorded.insert(self.step, answer);
        self.ticked.clear();
        text.clear();
        let count = self.active().map_or(0, |entry| {
            usize::try_from(entry.questions.len()).unwrap_or(usize::MAX)
        });
        if self.step + 1 < count {
            self.step += 1;
            return None;
        }
        let item = self.active()?.item.clone();
        let answers = (0..count)
            .map(|index| self.recorded.get(&index).cloned())
            .collect();
        self.prepare(item, answers)
    }

    fn prepare(
        &mut self,
        item: ItemId,
        answers: Vec<Option<QuestionAnswer>>,
    ) -> Option<(ItemId, Answer)> {
        if self.sending.is_some() {
            return None;
        }
        self.payloads.insert(item.clone(), answers.clone());
        self.error = None;
        self.sending = Some(item.clone());
        Some((item, Answer::Question { answers }))
    }

    pub(crate) fn can_go_back(&self) -> bool {
        self.step > 0 && self.sending.is_none()
    }

    pub(crate) fn back(&mut self, text: &mut String) -> bool {
        if !self.can_go_back() {
            return false;
        }
        self.step -= 1;
        self.error = None;
        self.ticked.clear();
        text.clear();
        match self.recorded.get(&self.step) {
            Some(QuestionAnswer::Choices { ids }) => {
                self.ticked = ids.inline().map(<[String]>::to_vec).unwrap_or_default();
            }
            Some(QuestionAnswer::Text { text: answer }) => answer.clone_into(text),
            None => {}
        }
        true
    }

    pub(crate) fn dismiss(&mut self, text: &mut String) -> Option<(ItemId, Answer)> {
        if self.sending.is_some() {
            return None;
        }
        let entry = self.active()?;
        let item = entry.item.clone();
        let count = usize::try_from(entry.questions.len()).unwrap_or(usize::MAX);
        text.clear();
        self.prepare(item, vec![None; count])
    }

    pub(crate) fn retry(&mut self) -> Option<(ItemId, Answer)> {
        let item = self.active()?.item.clone();
        let answers = self.payloads.get(&item)?.clone();
        self.prepare(item, answers)
    }

    pub(crate) async fn send(
        client: &ForgeClient,
        item: &ItemId,
        answer: &Answer,
    ) -> Result<(), editor_core::client::CallError> {
        client.answer(item, answer).await
    }

    pub(crate) fn settled(
        &mut self,
        item: &ItemId,
        error: Option<editor_core::client::CallError>,
        text: &mut String,
    ) {
        if self.sending.as_ref() != Some(item) {
            return;
        }
        self.sending = None;
        match error {
            None => {
                self.answered.push(item.clone());
                self.drafts.remove(item);
                self.payloads.remove(item);
                self.active = None;
                self.error = None;
                self.step = 0;
                self.recorded.clear();
                self.ticked.clear();
                self.load(text);
            }
            Some(error) => {
                let error = diagnostics::chain(&error);
                tracing::warn!(%item, %error, "sending a questionnaire answer failed");
                self.error = Some(format!("Couldn't send your answer: {error}"));
            }
        }
        self.sync_mode(text);
    }
}

#[cfg(test)]
#[path = "../../tests/composer/questionnaire.rs"]
mod tests;

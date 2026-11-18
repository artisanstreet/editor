use super::{AnswerCompletion, ComposerDrain, ComposerEffect, ComposerOwner, ComposerVisible};

impl ComposerOwner {
    pub fn answer_wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.answers.wake_mailbox()
    }

    pub(super) fn send_answer(
        &self,
        host: &editor_core::HostData,
        answer: Option<(artisan_api::ItemId, artisan_api::Answer)>,
        generation: u64,
    ) {
        let Some((item, answer)) = answer else {
            return;
        };
        let client = host.client.clone();
        let host_id = host.id.clone();
        let instance = host.instance();
        let sender = self.answers.sender();
        editor_core::runtime::runtime().spawn(async move {
            let result =
                super::super::questionnaire::QuestionnairePanel::send(&client, &item, &answer)
                    .await;
            let _ = sender
                .send(AnswerCompletion {
                    host: host_id,
                    instance,
                    generation,
                    item,
                    result,
                })
                .await;
        });
    }

    pub fn drain_answers(&mut self, core: &editor_core::Core) -> Vec<ComposerEffect> {
        let mut effects = Vec::new();
        for completion in self.answers.drain() {
            let mut composer = self.controller.borrow_mut();
            if composer.target.host() != &completion.host
                || composer.generation() != completion.generation
                || composer.questionnaire.sending.as_ref() != Some(&completion.item)
                || core
                    .host_if_instance(&completion.host, completion.instance)
                    .is_none()
            {
                continue;
            }
            let mut text = composer.editor.borrow().text().to_owned();
            composer
                .questionnaire
                .settled(&completion.item, completion.result.err(), &mut text);
            composer.editor.borrow_mut().set_text(&text);
            effects.push(ComposerEffect::EditorChanged);
        }
        effects
    }

    pub fn drain_async(
        &mut self,
        core: &editor_core::Core,
        assets: &aqua::images::ImageAssets,
    ) -> ComposerDrain {
        let revision = self.controller.borrow().editor.borrow().paint_revision();
        let events = {
            let mut composer = self.controller.borrow_mut();
            composer.drain_turn_actions(core);
            composer.drain_attachments(core, assets);
            composer.sync(core)
        };
        let mut effects = self.drain_answers(core);
        {
            let mut composer = self.controller.borrow_mut();
            let host = composer.target.host().clone();
            effects.extend(
                composer
                    .take_failed_stops()
                    .into_iter()
                    .map(|thread| ComposerEffect::StopFailed(host.clone(), thread)),
            );
        }
        let edited_here = self.controller.borrow().editor.borrow().paint_revision() != revision;
        let visible = ComposerVisible::capture(&self.controller.borrow());
        let changed = edited_here || !self.last_visible.borrow().same_chrome(&visible);
        *self.last_visible.borrow_mut() = visible;
        ComposerDrain {
            events,
            effects,
            changed,
        }
    }
}

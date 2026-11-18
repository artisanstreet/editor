use super::{
    ComposerAction, ComposerEffect, ComposerOwner, ComposerVisible, MODEL_LIST_HEIGHT,
    axis_options, choose_axis_config,
};

impl ComposerOwner {
    pub fn dispatch_with_change(
        &mut self,
        core: &mut editor_core::Core,
        action: ComposerAction,
        question: Option<&artisan_api::Question>,
    ) -> (Vec<ComposerEffect>, bool) {
        let valid = {
            let composer = self.controller.borrow();
            core.host(composer.target.host())
                .is_some_and(|host| composer.matches_host(host))
        };
        if !valid {
            return (Vec::new(), false);
        }
        let before = ComposerVisible::capture(&self.controller.borrow());
        let tooltip_before = self.tooltip.borrow().revision();
        let typed = matches!(
            action,
            ComposerAction::Editor(
                editor_ui::text::mount::EditorEvent::Changed
                    | editor_ui::text::mount::EditorEvent::SelectionChanged { .. }
                    | editor_ui::text::mount::EditorEvent::PresentationChanged
            )
        );
        let effects = self.dispatch(core, action, question);
        let after = ComposerVisible::capture(&self.controller.borrow());
        let changed = match typed {
            true => !self.last_visible.borrow().same_chrome(&after) || !before.same_chrome(&after),
            false => before != after || *self.last_visible.borrow() != after,
        } || tooltip_before != self.tooltip.borrow().revision()
            || self.model_scroll.borrow().jumping();
        *self.last_visible.borrow_mut() = after;
        (effects, changed)
    }

    fn dispatch(
        &mut self,
        core: &mut editor_core::Core,
        action: ComposerAction,
        question: Option<&artisan_api::Question>,
    ) -> Vec<ComposerEffect> {
        use editor_ui::text::mount::EditorEvent;
        let mut effects = Vec::new();
        let mut composer = self.controller.borrow_mut();
        let Some(host) = core.host(composer.target.host()) else {
            return effects;
        };
        if !composer.matches_host(host) {
            return effects;
        }
        composer.picker.refresh_settings(core, host);
        match action {
            ComposerAction::TooltipHover {
                instance,
                generation,
                anchor,
                content,
                hovered,
            } => {
                if composer.host_instance() == Some(instance)
                    && composer.generation() == generation
                    && composer.picker.menu == Some(super::super::picker::Menu::Model)
                {
                    let mut tooltip = self.tooltip.borrow_mut();
                    if hovered {
                        if let Some(content) = content {
                            tooltip.begin(content, std::time::Instant::now());
                        } else {
                            tooltip.clear();
                        }
                    } else {
                        tooltip.clear_for(&anchor);
                    }
                }
            }
            ComposerAction::DroppedPaths {
                paths,
                instance,
                generation,
            } => {
                if composer.host_instance() == Some(instance)
                    && composer.generation() == generation
                    && composer.draft_editable()
                {
                    composer.add_paths(paths);
                }
            }
            ComposerAction::DroppedThread(thread) => {
                if &thread.host == composer.target.host()
                    && artisan_mentions::valid_thread_title(&thread.title)
                {
                    let (at, lead) = {
                        let editor = composer.editor.borrow();
                        let text = editor.text();
                        let at = editor.caret().unwrap_or(text.len());
                        let spaced = text[..at]
                            .chars()
                            .next_back()
                            .is_none_or(char::is_whitespace);
                        (at, if spaced { "" } else { " " })
                    };
                    let mention = format!("{lead}@{} ", thread.title);
                    composer.editor.borrow_mut().replace(at..at, &mention);
                    composer.edited();
                    composer.sync_mentions();
                    effects.push(ComposerEffect::EditorChanged);
                    effects.push(ComposerEffect::FocusEditor);
                }
            }
            ComposerAction::Editor(EditorEvent::Changed) => {
                if !composer.questionnaire.writing_answer() {
                    composer.edited();
                }
                composer.sync_mentions();
            }
            ComposerAction::Editor(EditorEvent::SelectionChanged { .. }) => {
                composer.sync_mentions()
            }
            ComposerAction::Editor(EditorEvent::PresentationChanged) => {}
            ComposerAction::Editor(EditorEvent::List(key)) => {
                composer.mention_key(key);
                effects.push(ComposerEffect::EditorChanged);
            }
            ComposerAction::Editor(EditorEvent::PastedFiles { images, paths }) => {
                for image in images {
                    composer.add_image(
                        super::super::attachment::Source::Clipboard(image),
                        "Pasted image".into(),
                    );
                }
                composer.add_paths(paths);
            }
            ComposerAction::Editor(EditorEvent::Submitted) | ComposerAction::Send => {
                if composer.questionnaire.writing_answer() {
                    let mut text = composer.editor.borrow().text().to_owned();
                    let answer_text = text.trim().to_owned();
                    if !answer_text.is_empty() && composer.questionnaire.sending.is_none() {
                        let answer = composer.questionnaire.answer(
                            artisan_api::QuestionAnswer::Text { text: answer_text },
                            question.is_some(),
                            &mut text,
                        );
                        composer.editor.borrow_mut().set_text(&text);
                        self.send_answer(host, answer, composer.generation());
                        effects.push(ComposerEffect::EditorChanged);
                    }
                } else if composer.send(host) {
                    effects.push(ComposerEffect::EditorChanged);
                }
                effects.push(ComposerEffect::FocusEditor);
            }
            ComposerAction::Stop => {
                if let Some(thread) = composer.stop(host) {
                    effects.push(ComposerEffect::Stopping(host.id.clone(), thread));
                }
                effects.push(ComposerEffect::FocusEditor);
            }
            ComposerAction::Continue => {
                composer.continue_turn(host);
                effects.push(ComposerEffect::FocusEditor);
            }
            ComposerAction::ClearGoal => composer.clear_goal(host),
            ComposerAction::Unpark => composer.unpark(host),
            ComposerAction::ToggleModel => {
                self.tooltip.borrow_mut().clear();
                if composer.picker.menu == Some(super::super::picker::Menu::Model) {
                    composer.picker.menu = None;
                } else {
                    composer.open_picker(super::super::picker::Menu::Model, host);
                }
            }
            ComposerAction::ClosePicker => {
                self.tooltip.borrow_mut().clear();
                composer.picker.menu = None;
                composer.picker.axis = None;
            }
            ComposerAction::DismissMentions => {
                composer.mentions.dismiss();
                composer.sync_mentions();
            }
            ComposerAction::ToggleProject => {
                self.tooltip.borrow_mut().clear();
                if composer.picker.menu == Some(super::super::picker::Menu::Project) {
                    composer.picker.menu = None;
                } else {
                    composer.open_picker(super::super::picker::Menu::Project, host);
                }
            }
            ComposerAction::ChooseModel { engine, id } => {
                self.tooltip.borrow_mut().clear();
                let config = composer.choose_model(host, engine, &id);
                if let Some(config) = &config {
                    composer.apply_permissions(host, config.permissions);
                }
                let host_id = host.id.clone();
                let instance = host.instance();
                if let (Some(config), Some(host)) =
                    (config, core.host_mut_if_instance(&host_id, instance))
                {
                    host.remember_model_config(config);
                }
            }
            ComposerAction::OpenEngineSettings(engine) => {
                composer.picker.menu = None;
                core.navigate(editor_core::Screen::Settings(
                    editor_core::SettingsPage::Engine(engine),
                ));
            }
            ComposerAction::ToggleModelGroup { engine, name, open } => {
                composer.picker.toggle_group(engine, name.clone(), open);
                composer.refresh_picker(host);
                let host_id = host.id.clone();
                let instance = host.instance();
                if let Some(host) = core.host_mut_if_instance(&host_id, instance) {
                    host.remember_model_group(engine, name, !open);
                }
            }
            ComposerAction::JumpEngine(engine) => {
                self.tooltip.borrow_mut().clear();
                let layout = super::super::model_list::ListLayout::new(&composer.picker.models);
                if let Some(to) = layout.anchor(&composer.picker.models, engine, MODEL_LIST_HEIGHT)
                {
                    self.model_scroll.borrow_mut().jump(engine, to);
                }
            }
            ComposerAction::SelectPane(pane) => {
                self.tooltip.borrow_mut().clear();
                composer.picker.axis = pane;
                composer.picker.axis_cursor = None;
            }
            ComposerAction::AxisPointer { axis, index } => {
                if composer.picker.axis == Some(axis) {
                    composer.picker.axis_cursor =
                        Some(super::super::picker::AxisCursor::Pointer(index));
                }
            }
            ComposerAction::AxisKey { axis, key } => {
                if composer.picker.axis == Some(axis) {
                    if let Some(options) = axis_options(&composer, host, axis) {
                        let selected = options
                            .iter()
                            .position(|option| option.selected)
                            .unwrap_or(0);
                        match composer.picker.axis_key(key, selected, options.len()) {
                            super::super::picker::AxisKeyResult::Choose(index) => {
                                self.tooltip.borrow_mut().clear();
                                if let Some(option) = options.into_iter().nth(index) {
                                    if composer.metadata_ready(host) && composer.draft_editable() {
                                        let host_id = host.id.clone();
                                        choose_axis_config(
                                            &mut composer,
                                            core,
                                            &host_id,
                                            option.apply,
                                        );
                                    }
                                }
                            }
                            super::super::picker::AxisKeyResult::Dismiss => {
                                self.tooltip.borrow_mut().clear();
                                composer.picker.axis = None;
                            }
                            _ => {}
                        }
                    }
                }
            }
            ComposerAction::ChooseAxis { axis, index } => {
                if !composer.metadata_ready(host) || !composer.draft_editable() {
                    return effects;
                }
                if let Some(option) = axis_options(&composer, host, axis)
                    .and_then(|options| options.into_iter().nth(index))
                {
                    self.tooltip.borrow_mut().clear();
                    let host_id = host.id.clone();
                    choose_axis_config(&mut composer, core, &host_id, option.apply);
                }
            }
            ComposerAction::ChooseProject(id) => composer.choose_project(id),
            ComposerAction::NewProject => {
                composer.picker.menu = None;
                effects.push(ComposerEffect::NewProject);
            }
            ComposerAction::HoverMention(index, inside) => composer.mentions.hover(index, inside),
            ComposerAction::SelectMention(index) => {
                let replacement = {
                    let editor = composer.editor.borrow();
                    composer.mentions.replacement(editor.text(), index)
                };
                if let Some((range, text)) = replacement {
                    composer.editor.borrow_mut().replace(range, &text);
                    composer.edited();
                    composer.sync_mentions();
                    effects.push(ComposerEffect::EditorChanged);
                    effects.push(ComposerEffect::FocusEditor);
                }
            }
            ComposerAction::RemoveImage(key) => composer.remove_image(key),
            ComposerAction::RetryImage(key) => {
                if let Some(attachments) = &mut composer.attachments {
                    attachments.retry_preview(key);
                }
            }
            ComposerAction::RemoveFile(key) => composer.remove_file(key),
            ComposerAction::OpenImage(key) => effects.push(ComposerEffect::OpenImage(key)),
            ComposerAction::RetryDraft => {
                if let Some(session) = &composer.session {
                    session.retry();
                }
            }
            ComposerAction::KeepDraft => {
                if let Some(session) = &composer.session {
                    session.keep_local();
                }
            }
            ComposerAction::LoadOtherDraft => {
                if let Some(session) = &composer.session {
                    session.use_remote();
                }
            }
            ComposerAction::RetryModelChoice => {
                let host_id = host.id.clone();
                let instance = host.instance();
                if let Some(host) = core.host_mut_if_instance(&host_id, instance) {
                    host.retry_preferences();
                }
            }
            ComposerAction::RetryThreadSettings => {
                effects.push(ComposerEffect::RetryThreadSettings)
            }
            ComposerAction::DismissNotice => {
                composer.notice = None;
                if let Some(attachments) = &mut composer.attachments {
                    attachments.notice = None;
                }
            }
            ComposerAction::CloseNote => {
                composer
                    .strip
                    .close_note(std::time::Instant::now(), self.reduced_motion);
            }
            ComposerAction::OpenQuestionSource(source) => {
                effects.push(ComposerEffect::OpenQuestionSource(source))
            }
            ComposerAction::RetryQuestionPage => effects.push(ComposerEffect::RetryQuestionPage),
            ComposerAction::ActivateQuestionnaire(item) => {
                let mut text = composer.editor.borrow().text().to_owned();
                composer.questionnaire.activate(item, &mut text);
                composer.editor.borrow_mut().set_text(&text);
                effects.push(ComposerEffect::EditorChanged);
            }
            ComposerAction::DismissQuestionnaire => {
                let mut text = composer.editor.borrow().text().to_owned();
                let answer = composer.questionnaire.dismiss(&mut text);
                composer.editor.borrow_mut().set_text(&text);
                self.send_answer(host, answer, composer.generation());
                effects.push(ComposerEffect::EditorChanged);
            }
            ComposerAction::QuestionBack => {
                let mut text = composer.editor.borrow().text().to_owned();
                if composer.questionnaire.back(&mut text) {
                    composer.editor.borrow_mut().set_text(&text);
                    effects.push(ComposerEffect::EditorChanged);
                }
            }
            ComposerAction::RetryQuestionnaire => {
                let answer = composer.questionnaire.retry();
                self.send_answer(host, answer, composer.generation());
            }
            ComposerAction::ChooseQuestionOption {
                item,
                question: expected,
                step,
                id,
                multi,
            } => {
                if composer.questionnaire.sending.is_none()
                    && composer.questionnaire.step == step
                    && composer
                        .questionnaire
                        .active()
                        .is_some_and(|active| active.item == item)
                    && question.is_some_and(|question| question.id == expected)
                {
                    if multi {
                        composer.questionnaire.toggle(id);
                    } else {
                        let mut text = composer.editor.borrow().text().to_owned();
                        let answer = composer.questionnaire.answer(
                            artisan_api::QuestionAnswer::Choices {
                                ids: vec![id].into(),
                            },
                            true,
                            &mut text,
                        );
                        composer.editor.borrow_mut().set_text(&text);
                        self.send_answer(host, answer, composer.generation());
                        effects.push(ComposerEffect::EditorChanged);
                    }
                }
            }
            ComposerAction::ContinueQuestionnaire => {
                if !composer.questionnaire.ticked.is_empty() {
                    let ids = std::mem::take(&mut composer.questionnaire.ticked);
                    let mut text = composer.editor.borrow().text().to_owned();
                    let answer = composer.questionnaire.answer(
                        artisan_api::QuestionAnswer::Choices { ids: ids.into() },
                        question.is_some(),
                        &mut text,
                    );
                    composer.editor.borrow_mut().set_text(&text);
                    self.send_answer(host, answer, composer.generation());
                    effects.push(ComposerEffect::EditorChanged);
                }
            }
        }
        effects
    }
}

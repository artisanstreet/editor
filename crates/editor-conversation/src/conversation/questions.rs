use editor_sources::SourceProviderExt;
use editor_composer::questionnaire::QuestionnairePanel;
use editor_core::{Core, HostId, HostInstance, RangeState};
use editor_sources::{CollectionBrowser, page_record};
use artisan_api::{Collection, CollectionRef, ContentRef, ItemId, Question, QuestionOption};
use std::{cell::RefCell, rc::Rc};

struct Owner<T: serde::de::DeserializeOwned + Send + 'static> {
    host: HostId,
    instance: HostInstance,
    item: ItemId,
    source: CollectionRef,
    browser: Rc<RefCell<CollectionBrowser<T>>>,
}

impl<T: serde::de::DeserializeOwned + Send + 'static> Owner<T> {
    fn matches(
        &self,
        host: &HostId,
        instance: HostInstance,
        item: &ItemId,
        source: &CollectionRef,
    ) -> bool {
        &self.host == host
            && self.instance == instance
            && &self.item == item
            && &self.source == source
    }
}

struct QuestionOwner {
    host: HostId,
    instance: HostInstance,
    item: ItemId,
    source: CollectionRef,
    browser: CollectionBrowser<Question>,
}

impl QuestionOwner {
    fn matches(
        &self,
        host: &HostId,
        instance: HostInstance,
        item: &ItemId,
        source: &CollectionRef,
    ) -> bool {
        &self.host == host
            && self.instance == instance
            && &self.item == item
            && &self.source == source
    }
}

#[derive(Default)]
pub struct QuestionPages {
    questions: Option<QuestionOwner>,
    options: Option<Owner<QuestionOption>>,
    option_question: Option<(usize, String)>,
    source_slot: Option<(ItemId, usize)>,
    pub source_view: Option<editor_sources::ContentText>,
}

impl QuestionPages {
    pub fn sync(
        &mut self,
        core: &Core,
        host_id: Option<&HostId>,
        instance: Option<HostInstance>,
        panel: &QuestionnairePanel,
    ) {
        let (Some(host_id), Some(instance), Some(active)) = (host_id, instance, panel.active())
        else {
            self.clear();
            return;
        };
        let Some(host) = core.host_if_instance(host_id, instance) else {
            self.clear();
            return;
        };
        if self
            .source_slot
            .as_ref()
            .is_some_and(|slot| slot != &(active.item.clone(), panel.step))
        {
            self.source_slot = None;
            self.source_view = None;
        }
        match &active.questions {
            Collection::Inline(_) => self.questions = None,
            Collection::Stored(source) => {
                if self
                    .questions
                    .as_ref()
                    .is_none_or(|owner| !owner.matches(host_id, instance, &active.item, source))
                {
                    self.questions = Some(QuestionOwner {
                        host: host_id.clone(),
                        instance,
                        item: active.item.clone(),
                        source: source.clone(),
                        browser: CollectionBrowser::new(host, source.clone()),
                    });
                }
                if let Some(owner) = &mut self.questions {
                    owner.browser.request(core, panel.step as u64);
                }
            }
        }
        let option = self
            .current_question(panel)
            .and_then(|question| match &question.options {
                Collection::Stored(source) => Some((question.id.clone(), source.clone())),
                Collection::Inline(_) => None,
            });
        match option {
            Some((question, source)) => {
                if self.option_question.as_ref() != Some(&(panel.step, question.clone()))
                    || self.options.as_ref().is_none_or(|owner| {
                        !owner.matches(host_id, instance, &active.item, &source)
                    })
                {
                    self.options = Some(Owner {
                        host: host_id.clone(),
                        instance,
                        item: active.item.clone(),
                        source: source.clone(),
                        browser: Rc::new(RefCell::new(CollectionBrowser::new(host, source))),
                    });
                    self.option_question = Some((panel.step, question));
                }
            }
            None => {
                self.options = None;
                self.option_question = None;
            }
        }
    }

    pub fn clear(&mut self) {
        self.questions = None;
        self.options = None;
        self.option_question = None;
        self.source_slot = None;
        self.source_view = None;
    }

    pub fn current_question<'a>(&'a self, panel: &'a QuestionnairePanel) -> Option<&'a Question> {
        let active = panel.active()?;
        match &active.questions {
            Collection::Inline(questions) => questions.get(panel.step),
            Collection::Stored(source) => {
                let owner = self
                    .questions
                    .as_ref()
                    .filter(|owner| owner.item == active.item && &owner.source == source)?;
                let index = panel.step as u64;
                page_record(source, owner.browser.window().page_for(index)?, index)
            }
        }
    }

    pub fn question_error(&self) -> Option<&str> {
        match self.questions.as_ref()?.browser.window().state() {
            RangeState::Failed(error) => Some(error),
            RangeState::Loading | RangeState::Ready(_) => None,
        }
    }

    pub fn retry_question(&mut self, core: &Core) {
        if let Some(owner) = &mut self.questions {
            owner.browser.retry(core);
        }
    }

    pub fn question_wake(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        Some(self.questions.as_ref()?.browser.window().wake_mailbox())
    }

    pub fn options_browser(&self) -> Option<Rc<RefCell<CollectionBrowser<QuestionOption>>>> {
        Some(Rc::clone(&self.options.as_ref()?.browser))
    }

    pub fn drain(&mut self, core: &Core) -> bool {
        let questions = self
            .questions
            .as_mut()
            .is_some_and(|owner| owner.browser.drain(core));
        let options = self
            .options
            .as_mut()
            .is_some_and(|owner| owner.browser.borrow_mut().drain(core));
        questions || options
    }

    pub fn open_source(
        &mut self,
        core: &Core,
        host: &HostId,
        instance: HostInstance,
        panel: &QuestionnairePanel,
        source: ContentRef,
    ) -> bool {
        let Some(active) = panel.active() else {
            return false;
        };
        let Some(host) = core.host_if_instance(host, instance) else {
            return false;
        };
        let Some(question) = self.current_question(panel) else {
            return false;
        };
        let is_source = |text: &artisan_api::TextContent| matches!(text, artisan_api::TextContent::Stored(reference) if reference == &source);
        let belongs = is_source(&question.text)
            || question.options.inline().is_some_and(|options| {
                options
                    .iter()
                    .any(|option| option.description.as_ref().is_some_and(&is_source))
            });
        if !belongs {
            return false;
        }
        self.source_slot = Some((active.item.clone(), panel.step));
        self.source_view = Some(editor_sources::ContentText::new(host, source));
        true
    }

    pub fn option_action(
        &self,
        panel: &QuestionnairePanel,
        index: u64,
    ) -> Option<editor_composer::view::ComposerAction> {
        let active = panel.active()?;
        let question = self.current_question(panel)?;
        let owner = self.options.as_ref()?;
        let source = question.options.stored()?;
        if owner.item != active.item
            || &owner.source != source
            || self.option_question.as_ref() != Some(&(panel.step, question.id.clone()))
        {
            return None;
        }
        let browser = owner.browser.borrow();
        let option = page_record(source, browser.window().page_for(index)?, index)?;
        Some(
            editor_composer::view::ComposerAction::ChooseQuestionOption {
                item: active.item.clone(),
                question: question.id.clone(),
                step: panel.step,
                id: option.id.clone(),
                multi: question.multi_select,
            },
        )
    }
}

pub struct OptionAction {
    host: HostId,
    instance: HostInstance,
    item: ItemId,
    step: usize,
    source: CollectionRef,
    action: editor_sources::CollectionAction<QuestionOption>,
}

pub fn option_preview(
    option: &QuestionOption,
    panel: &QuestionnairePanel,
) -> editor_sources::CollectionPreview {
    editor_sources::CollectionPreview {
        label: option.label.clone(),
        detail: option.description.clone(),
        selected: panel.ticked.contains(&option.id),
        disabled: panel.sending.is_some(),
    }
}

pub fn question_preview(question: &Question, index: u64) -> editor_sources::CollectionPreview {
    let preview = match &question.text {
        artisan_api::TextContent::Inline(text) => text.chars().take(160).collect(),
        artisan_api::TextContent::Stored(_) => "Read content".to_owned(),
    };
    editor_sources::CollectionPreview {
        label: format!("{}. {preview}", index + 1),
        detail: Some(question.text.clone()),
        selected: false,
        disabled: false,
    }
}

#[derive(Default)]
pub struct QuestionCollection {
    owner: Option<Owner<Question>>,
}

pub struct QuestionCollectionAction {
    host: HostId,
    instance: HostInstance,
    item: ItemId,
    source: CollectionRef,
    action: editor_sources::CollectionAction<Question>,
}

impl QuestionCollectionAction {
    pub fn matches_item(&self, item: &artisan_api::Item) -> bool {
        item.id == self.item
            && matches!(&item.kind, artisan_api::ItemKind::Question { questions: Collection::Stored(source), .. } if source == &self.source)
    }
}

impl QuestionCollection {
    pub fn identity(&self) -> Option<(ItemId, CollectionRef)> {
        self.owner
            .as_ref()
            .map(|owner| (owner.item.clone(), owner.source.clone()))
    }
    pub fn dispatch_changed(&mut self, core: &Core, action: QuestionCollectionAction) -> bool {
        let before = self.browser().map(|browser| browser.borrow().view_state());
        self.dispatch(core, action);
        let after = self.browser().map(|browser| browser.borrow().view_state());
        before != after
    }

    pub fn drain_changed(&mut self, core: &Core) -> bool {
        self.browser().is_some_and(|browser| {
            editor_sources::drain_collection_changed(&mut browser.borrow_mut(), core)
        })
    }
    pub fn open_provider(
        &mut self,
        provider: &editor_core::host::ScopedSourceProvider,
        item: &ItemId,
        source: &CollectionRef,
    ) -> bool {
        if !provider.is_available() {
            return false;
        }
        if let Some(owner) = self.owner.as_mut()
            && &owner.host == provider.host()
            && owner.instance == provider.instance()
            && &owner.item == item
        {
            if owner.source != *source {
                owner.browser.borrow_mut().set_source(source.clone());
                owner.source = source.clone();
            }
            return true;
        }
        if self
            .owner
            .as_ref()
            .is_none_or(|owner| !owner.matches(provider.host(), provider.instance(), item, source))
        {
            let Some(browser) = provider.open_collection(source.clone()) else {
                return false;
            };
            self.owner = Some(Owner {
                host: provider.host().clone(),
                instance: provider.instance(),
                item: item.clone(),
                source: source.clone(),
                browser: Rc::new(RefCell::new(browser)),
            });
        }
        true
    }
    pub fn retain(
        &mut self,
        core: &Core,
        item: impl FnOnce(&ItemId) -> Option<std::sync::Arc<artisan_api::Item>>,
    ) {
        let Some(owner) = self.owner.as_mut() else {
            return;
        };
        if core.host_if_instance(&owner.host, owner.instance).is_none() {
            self.clear();
            return;
        }
        let Some(item) = item(&owner.item) else {
            self.clear();
            return;
        };
        let artisan_api::ItemKind::Question {
            questions: Collection::Stored(source),
            ..
        } = &item.kind
        else {
            self.clear();
            return;
        };
        if owner.source != *source {
            owner.browser.borrow_mut().set_source(source.clone());
            owner.source = source.clone();
        }
    }

    pub fn open(
        &mut self,
        core: &Core,
        host: &HostId,
        instance: HostInstance,
        item: &ItemId,
        source: &CollectionRef,
    ) -> bool {
        let Some(host_data) = core.host_if_instance(host, instance) else {
            self.owner = None;
            return false;
        };
        if self
            .owner
            .as_ref()
            .is_none_or(|owner| !owner.matches(host, instance, item, source))
        {
            self.owner = Some(Owner {
                host: host.clone(),
                instance,
                item: item.clone(),
                source: source.clone(),
                browser: Rc::new(RefCell::new(CollectionBrowser::new(
                    host_data,
                    source.clone(),
                ))),
            });
        }
        true
    }

    pub fn clear(&mut self) {
        self.owner = None;
    }

    pub fn browser(&self) -> Option<Rc<RefCell<CollectionBrowser<Question>>>> {
        Some(Rc::clone(&self.owner.as_ref()?.browser))
    }

    pub fn dispatch(&mut self, core: &Core, action: QuestionCollectionAction) {
        use editor_sources::CollectionAction;
        let Some(owner) = self.owner.as_mut() else {
            return;
        };
        if !owner.matches(&action.host, action.instance, &action.item, &action.source)
            || core
                .host_if_instance(&action.host, action.instance)
                .is_none()
        {
            return;
        }
        let mut browser = owner.browser.borrow_mut();
        match action.action {
            CollectionAction::Request(index) => browser.request(core, index),
            CollectionAction::Retry => browser.retry(core),
            CollectionAction::Back => browser.clear_detail(),
            CollectionAction::Read { page, index } => {
                browser.open_detail(core, page, index, question_preview);
            }
            CollectionAction::Choose { page, index } => {
                browser.open_detail(core, page, index, question_preview);
            }
        }
    }

    pub fn view(
        &self,
        theme: editor_ui::theme::ArtisanTheme,
        document: aqua_text::document::SelectionDocumentId,
        source_id: aqua_text::document::SelectionSourceId,
        stored_detail: Option<aqua_ui::element::Element>,
        emit: Rc<dyn Fn(QuestionCollectionAction)>,
    ) -> Result<Option<editor_sources::ContentView>, &'static str> {
        let Some(owner) = self.owner.as_ref() else {
            return Ok(None);
        };
        let browser = Rc::clone(&owner.browser);
        let host = owner.host.clone();
        let instance = owner.instance;
        let item = owner.item.clone();
        let source = owner.source.clone();
        let action = Rc::new(move |action| {
            emit(QuestionCollectionAction {
                host: host.clone(),
                instance,
                item: item.clone(),
                source: source.clone(),
                action,
            })
        });
        if browser.borrow().selected.is_some() {
            return editor_sources::collection_detail(
                &browser.borrow(),
                theme,
                None,
                document,
                source_id,
                stored_detail,
                action,
            )
            .map(Some);
        }
        Ok(Some(editor_sources::ContentView {
            element: editor_sources::collection_list(
                browser,
                "Questions",
                theme,
                None,
                Rc::new(question_preview),
                false,
                action,
            ),
            selection: None,
        }))
    }
}

impl QuestionPages {
    pub fn handle_option_action(
        &mut self,
        core: &Core,
        panel: &QuestionnairePanel,
        action: OptionAction,
    ) -> Option<editor_composer::view::ComposerAction> {
        use editor_sources::CollectionAction;
        let owner = self.options.as_ref()?;
        let active = panel.active()?;
        if core
            .host_if_instance(&action.host, action.instance)
            .is_none()
            || !owner.matches(&action.host, action.instance, &action.item, &action.source)
            || active.item != action.item
            || panel.step != action.step
            || self.current_question(panel)?.options.stored() != Some(&action.source)
        {
            return None;
        }
        match action.action {
            CollectionAction::Request(index) => {
                self.options
                    .as_mut()?
                    .browser
                    .borrow_mut()
                    .request(core, index);
                None
            }
            CollectionAction::Retry => {
                self.options.as_mut()?.browser.borrow_mut().retry(core);
                None
            }
            CollectionAction::Back => {
                self.options.as_mut()?.browser.borrow_mut().clear_detail();
                None
            }
            CollectionAction::Read { page, index } => {
                let owner = self.options.as_mut()?;
                owner
                    .browser
                    .borrow_mut()
                    .open_detail(core, page, index, |option, _| option_preview(option, panel));
                None
            }
            CollectionAction::Choose { page, index } => {
                let action = self.option_action(panel, index)?;
                let owner = self.options.as_ref()?;
                let valid = owner.browser.borrow().activate(
                    core,
                    &page,
                    index,
                    |option, _| option_preview(option, panel),
                    |_, _| {},
                );
                valid.then_some(action)
            }
        }
    }
}

pub fn stored_options(
    pages: &QuestionPages,
    panel: &QuestionnairePanel,
    theme: editor_ui::theme::ArtisanTheme,
    document: aqua_text::document::SelectionDocumentId,
    source_id: aqua_text::document::SelectionSourceId,
    stored_detail: Option<aqua_ui::element::Element>,
    dispatch: Rc<dyn Fn(OptionAction)>,
) -> Result<Option<editor_sources::ContentView>, &'static str> {
    let Some(owner) = pages.options.as_ref() else {
        return Ok(None);
    };
    let browser = Rc::clone(&owner.browser);
    let host = owner.host.clone();
    let instance = owner.instance;
    let item = owner.item.clone();
    let source = owner.source.clone();
    let step = panel.step;
    let dispatch: Rc<dyn Fn(editor_sources::CollectionAction<QuestionOption>)> =
        Rc::new(move |action| {
            dispatch(OptionAction {
                host: host.clone(),
                instance,
                item: item.clone(),
                step,
                source: source.clone(),
                action,
            });
        });
    if browser.borrow().selected.is_some() {
        return editor_sources::collection_detail(
            &browser.borrow(),
            theme,
            None,
            document,
            source_id,
            stored_detail,
            dispatch,
        )
        .map(Some);
    }
    let ticked = panel.ticked.clone();
    let sending = panel.sending.is_some();
    let present = Rc::new(
        move |option: &QuestionOption, _| editor_sources::CollectionPreview {
            label: option.label.clone(),
            detail: option.description.clone(),
            selected: ticked.contains(&option.id),
            disabled: sending,
        },
    );
    Ok(Some(editor_sources::ContentView {
        element: editor_sources::collection_list(
            browser, "Choices", theme, None, present, true, dispatch,
        ),
        selection: None,
    }))
}

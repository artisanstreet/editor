use crate::conversation::fold::RowKey;
use crate::conversation::presentation::PresentationScope;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::{Rc, Weak};

pub struct RowOwner {
    pub(crate) motion: RefCell<super::motion::RowMotion>,
    pub(crate) reveal: RefCell<Option<Rc<RefCell<super::reveal::MountedReveal>>>>,
    pub(crate) sources: RefCell<crate::conversation::content::SourceOwners>,
    pub(crate) questions: RefCell<crate::conversation::questions::QuestionCollection>,
    pub(crate) batches: RefCell<super::resources::ResourceBatches>,
    pub(crate) source_ids:
        RefCell<HashMap<super::resources::ResourceKey, aqua_text::document::SelectionSourceId>>,
    pub(crate) stored_leaf: RefCell<Option<Rc<crate::conversation::stored::StoredLeafLease>>>,
    cleanup: Rc<RefCell<Vec<super::resources::BatchRelease>>>,
    identity: Option<(RowKey, PresentationScope)>,
}

impl Default for RowOwner {
    fn default() -> Self {
        Self {
            motion: RefCell::new(super::motion::RowMotion::default()),
            reveal: RefCell::new(None),
            sources: RefCell::new(Default::default()),
            questions: RefCell::new(Default::default()),
            batches: RefCell::new(Default::default()),
            source_ids: RefCell::new(HashMap::new()),
            stored_leaf: RefCell::new(None),
            cleanup: Rc::default(),
            identity: None,
        }
    }
}

impl Drop for RowOwner {
    fn drop(&mut self) {
        self.cleanup
            .borrow_mut()
            .extend(self.batches.get_mut().take_releases());
    }
}

impl RowOwner {
    #[cfg(feature = "test-support")]
    pub fn question_browser_for_tests(
        &self,
    ) -> Option<Rc<RefCell<editor_sources::CollectionBrowser<artisan_api::Question>>>> {
        self.questions.borrow().browser()
    }
    pub fn identity(&self) -> Option<(&RowKey, &PresentationScope)> {
        self.identity.as_ref().map(|(key, scope)| (key, scope))
    }

    pub(crate) fn retain_batches(&self, keep: &HashSet<super::resources::ResourceKey>) {
        self.cleanup
            .borrow_mut()
            .extend(self.batches.borrow_mut().retain_deferred(keep));
        self.source_ids
            .borrow_mut()
            .retain(|key, _| keep.contains(key));
    }
}

#[derive(Default)]
pub struct RowCache {
    scope: Option<PresentationScope>,
    rows: HashMap<RowKey, Rc<RowOwner>>,
    order: VecDeque<RowKey>,
    live: RefCell<Vec<Weak<RowOwner>>>,
    cleanup: Rc<RefCell<Vec<super::resources::BatchRelease>>>,
}

impl RowCache {
    pub const CAPACITY: usize = 128;

    pub fn rebind(&mut self, scope: Option<PresentationScope>) {
        if self.scope != scope {
            self.rows.clear();
            self.order.clear();
            self.scope = scope;
        }
    }

    pub fn scope_is(&self, scope: &PresentationScope) -> bool {
        self.scope.as_ref() == Some(scope)
    }

    pub fn candidate(&self, key: &RowKey) -> Rc<RowOwner> {
        if let Some(owner) = self.rows.get(key) {
            return owner.clone();
        }
        let mut owner = RowOwner::default();
        owner.cleanup = self.cleanup.clone();
        owner.identity = self
            .scope
            .as_ref()
            .map(|scope| (key.clone(), scope.clone()));
        let owner = Rc::new(owner);
        let mut live = self.live.borrow_mut();
        live.retain(|owner| owner.strong_count() > 0);
        live.push(Rc::downgrade(&owner));
        owner
    }

    pub fn live_owners(&self) -> Vec<Weak<RowOwner>> {
        let mut live = self.live.borrow_mut();
        live.retain(|owner| owner.strong_count() > 0);
        live.clone()
    }

    pub fn drain_cleanup(&self, app: &mut aqua::UiApp) {
        for release in std::mem::take(&mut *self.cleanup.borrow_mut()) {
            release.apply(app);
        }
    }

    pub fn commit(&mut self, key: RowKey, owner: Rc<RowOwner>) {
        self.order.retain(|cached| cached != &key);
        self.rows.insert(key.clone(), owner);
        self.order.push_back(key);
        while self.order.len() > Self::CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.rows.remove(&oldest);
            }
        }
    }

    pub fn retain_rows(&mut self, keys: &HashSet<RowKey>) {
        self.rows.retain(|key, _| keys.contains(key));
        self.order.retain(|key| keys.contains(key));
    }

    pub fn get(&self, key: &RowKey) -> Option<Rc<RowOwner>> {
        self.rows.get(key).cloned()
    }
}

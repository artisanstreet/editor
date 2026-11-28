use artisan_api::EngineId;
use editor_core::Problem;
use editor_ui::text::{Editor, Mode};
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap, HashSet},
    rc::Rc,
};

#[derive(Default)]
pub struct EnginesForm {
    pub(crate) host: Option<String>,
    pub login_codes: HashMap<EngineId, Rc<RefCell<Editor>>>,
    pub busy: BTreeSet<EngineId>,
    pub cancelling: BTreeSet<EngineId>,
    pub errors: HashMap<EngineId, Problem>,
    pub(crate) choosing_version: HashSet<EngineId>,
    pub(crate) api_key: Option<Rc<RefCell<Editor>>>,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lane {
    Action,
    Cancel,
}

#[derive(Clone, Debug)]
pub struct EngineRequest {
    pub(crate) host: String,
    engine: EngineId,
    lane: Lane,
    generation: u64,
}

impl EnginesForm {
    pub fn sync_host(&mut self, host: Option<String>) {
        if self.host == host {
            return;
        }
        self.host = host;
        self.generation += 1;
        self.login_codes.clear();
        self.busy.clear();
        self.cancelling.clear();
        self.errors.clear();
        self.choosing_version.clear();
        self.api_key = None;
    }

    pub(crate) fn begin(&mut self, engine: EngineId) -> Option<EngineRequest> {
        let host = self.host.clone()?;
        if self.busy.contains(&engine) {
            return None;
        }
        self.errors.remove(&engine);
        self.busy.insert(engine);
        Some(EngineRequest {
            host,
            engine,
            lane: Lane::Action,
            generation: self.generation,
        })
    }

    pub(crate) fn begin_cancel(&mut self, engine: EngineId) -> Option<EngineRequest> {
        let host = self.host.clone()?;
        if !self.cancelling.insert(engine) {
            return None;
        }
        self.errors.remove(&engine);
        Some(EngineRequest {
            host,
            engine,
            lane: Lane::Cancel,
            generation: self.generation,
        })
    }

    pub(crate) fn complete(&mut self, request: &EngineRequest, error: Option<Problem>) {
        if self.host.as_ref() != Some(&request.host) || self.generation != request.generation {
            return;
        }
        match request.lane {
            Lane::Action => self.busy.remove(&request.engine),
            Lane::Cancel => self.cancelling.remove(&request.engine),
        };
        if let Some(error) = error {
            self.errors.insert(request.engine, error);
        }
    }

    pub fn prepare(&mut self, engine: EngineId) {
        self.login_codes
            .entry(engine)
            .or_insert_with(|| Rc::new(RefCell::new(Editor::new("", Mode::SingleLine))));
    }

    pub(crate) fn open_api_key(&mut self) {
        self.api_key = Some(Rc::new(RefCell::new(Editor::new("", Mode::SingleLine))));
    }

    pub(crate) fn take_api_key(&mut self) -> Option<artisan_api::Secret> {
        if self.busy.contains(&EngineId::Codex) {
            return None;
        }
        let field = self.api_key.as_ref()?;
        let key = field.borrow().text().trim().to_owned();
        if key.is_empty() {
            self.errors
                .insert(EngineId::Codex, Problem::new("Enter a Codex API key."));
            return None;
        }
        self.api_key = None;
        Some(artisan_api::Secret::new(key))
    }

    pub fn login_code(&mut self, engine: EngineId) -> Option<String> {
        if self.busy.contains(&engine) {
            return None;
        }
        let code = self.login_codes.get_mut(&engine)?;
        let trimmed = code.borrow().text().trim().to_owned();
        if trimmed.is_empty() {
            self.errors.insert(
                engine,
                Problem::new("Paste the code the sign-in page showed you first."),
            );
            return None;
        }
        code.borrow_mut().set_text("");
        Some(trimmed)
    }
}

impl std::fmt::Debug for EnginesForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnginesForm")
            .field("host", &self.host)
            .field("busy", &self.busy)
            .field("cancelling", &self.cancelling)
            .finish_non_exhaustive()
    }
}

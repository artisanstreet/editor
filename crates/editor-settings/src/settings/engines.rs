use editor_ui::text::{Editor, Mode};
use artisan_api::EngineId;
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap, HashSet},
    rc::Rc,
};

#[derive(Default)]
pub struct EnginesForm {
    pub host: Option<String>,
    pub login_codes: HashMap<EngineId, Rc<RefCell<Editor>>>,
    pub busy: BTreeSet<EngineId>,
    pub errors: HashMap<EngineId, String>,
    pub choosing_version: HashSet<EngineId>,
    generation: u64,
}

#[derive(Clone, Debug)]
pub struct EngineRequest {
    pub host: String,
    pub engine: EngineId,
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
        self.errors.clear();
        self.choosing_version.clear();
    }

    pub fn begin(&mut self, engine: EngineId) -> Option<EngineRequest> {
        let host = self.host.clone()?;
        if self.busy.contains(&engine) {
            return None;
        }
        self.errors.remove(&engine);
        self.busy.insert(engine);
        Some(EngineRequest {
            host,
            engine,
            generation: self.generation,
        })
    }

    pub fn complete(&mut self, request: &EngineRequest, error: Option<String>) {
        if self.host.as_ref() != Some(&request.host) || self.generation != request.generation {
            return;
        }
        self.busy.remove(&request.engine);
        if let Some(error) = error {
            self.errors.insert(request.engine, error);
        }
    }

    pub fn prepare(&mut self, engine: EngineId) {
        self.login_codes
            .entry(engine)
            .or_insert_with(|| Rc::new(RefCell::new(Editor::new("", Mode::SingleLine))));
    }

    pub fn login_code(&mut self, engine: EngineId) -> Option<String> {
        let code = self.login_codes.get_mut(&engine)?;
        let trimmed = code.borrow().text().trim().to_owned();
        if trimmed.is_empty() {
            self.errors.insert(
                engine,
                "Paste the code the sign-in page showed you first.".to_owned(),
            );
            return None;
        }
        code.borrow_mut().set_text("");
        Some(trimmed)
    }
}

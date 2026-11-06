use aqua_protocol::{DialogKind, DialogOutcome, DialogSpec, RequestId};
use std::cell::RefCell;
use std::collections::HashMap;

type Route = Box<dyn FnOnce(DialogOutcome)>;

#[derive(Default)]
pub struct DialogRoutes {
    routes: RefCell<HashMap<RequestId, Route>>,
}

impl DialogRoutes {
    pub fn route(&self, request: RequestId, route: impl FnOnce(DialogOutcome) + 'static) {
        self.routes.borrow_mut().insert(request, Box::new(route));
    }

    pub fn complete(&self, request: RequestId, outcome: DialogOutcome) -> bool {
        let route = self.routes.borrow_mut().remove(&request);
        match route {
            Some(route) => {
                route(outcome);
                true
            }
            None => false,
        }
    }

    pub fn clear(&self) {
        self.routes.borrow_mut().clear();
    }
}

pub fn save_file_spec(file_name: &str) -> DialogSpec {
    DialogSpec {
        kind: DialogKind::SaveFile {
            suggested_name: file_name.to_owned(),
        },
        title: String::new(),
        filters: Vec::new(),
    }
}

pub fn chosen_path(outcome: DialogOutcome) -> Result<Option<std::path::PathBuf>, String> {
    match outcome {
        DialogOutcome::Chosen(paths) => Ok(paths.into_iter().next().map(Into::into)),
        DialogOutcome::Dismissed => Ok(None),
        DialogOutcome::Failed(message) => Err(message),
    }
}

use artisan_api::{CreateProject, DirListing, ProjectId};

#[derive(Default)]
pub struct FolderPicker {
    pub listing: Option<DirListing>,
    pub hidden: bool,
    pub loading: bool,
    pub creating: bool,
    pub error: Option<String>,
    request: Option<CreateProject>,
}

impl FolderPicker {
    pub fn rows(&self) -> Vec<usize> {
        self.listing
            .as_ref()
            .map(|listing| {
                listing
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| {
                        entry.is_dir && (self.hidden || !entry.name.starts_with('.'))
                    })
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn begin_browse(&mut self) -> bool {
        if self.creating {
            return false;
        }
        self.loading = true;
        self.error = None;
        true
    }
    pub fn finish_browse(&mut self, result: Result<DirListing, String>) {
        self.loading = false;
        match result {
            Ok(listing) => {
                self.listing = Some(listing);
                self.request = None;
            }
            Err(error) => self.error = Some(format!("Couldn't open this folder: {error}")),
        }
    }
    pub fn begin_create(&mut self) -> Option<CreateProject> {
        if self.creating || self.loading {
            return None;
        }
        let path = self.listing.as_ref()?.path.clone();
        let request = self
            .request
            .get_or_insert_with(|| CreateProject {
                id: ProjectId::new(),
                path,
                name: None,
            })
            .clone();
        self.creating = true;
        self.error = None;
        Some(request)
    }
    pub fn finish_create(&mut self, error: Option<String>) {
        self.creating = false;
        self.error = error.map(|error| format!("Couldn't add this folder: {error}"));
    }
}

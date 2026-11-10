use artisan_api::{GlobalPrompt, SavePrompt};

#[derive(Clone, Debug, Default)]
pub struct PromptForm {
    pub host: Option<String>,
    pub loaded: bool,
    pub saved: String,
    pub base_version: u64,
    pub newer: bool,
    pub pending: bool,
    pub error: Option<String>,
    generation: u64,
    edit_revision: u64,
}

#[derive(Clone, Debug)]
pub struct PromptRequest {
    pub host: String,
    pub save: SavePrompt,
    generation: u64,
    revision: u64,
}

impl PromptForm {
    pub fn changed(&mut self) {
        self.edit_revision = self.edit_revision.wrapping_add(1);
    }

    pub fn sync_host(&mut self, host: Option<String>) -> bool {
        let Some(host) = host else {
            self.host = None;
            self.loaded = false;
            self.generation = self.generation.wrapping_add(1);
            self.pending = false;
            return false;
        };
        if self.host.as_ref() == Some(&host) {
            return false;
        }
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
        self.host = Some(host);
        self.loaded = false;
        self.newer = false;
        self.error = None;
        self.base_version = 0;
        self.saved.clear();
        true
    }

    pub fn load(&mut self, prompt: &GlobalPrompt, text: &str, replace: bool) -> Option<String> {
        let replacement = if replace || text == self.saved {
            self.saved.clone_from(&prompt.text);
            self.base_version = prompt.version;
            self.newer = false;
            (text != prompt.text).then(|| prompt.text.clone())
        } else {
            if prompt.version != self.base_version {
                self.newer = true;
            }
            None
        };
        self.loaded = true;
        replacement
    }

    pub fn save(&mut self, text: &str) -> Option<PromptRequest> {
        let host = self.host.clone()?;
        if !self.loaded || self.pending {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.pending = true;
        self.error = None;
        Some(PromptRequest {
            host,
            save: SavePrompt {
                base_version: self.base_version,
                text: text.to_owned(),
            },
            generation: self.generation,
            revision: self.edit_revision,
        })
    }

    pub fn accepts(&self, request: &PromptRequest) -> bool {
        self.host.as_ref() == Some(&request.host) && self.generation == request.generation
    }

    pub fn complete(
        &mut self,
        request: &PromptRequest,
        result: Result<(u64, Option<GlobalPrompt>), String>,
        text: &str,
    ) -> Option<String> {
        if self.host.as_ref() != Some(&request.host) || self.generation != request.generation {
            return None;
        }
        self.pending = false;
        match result {
            Ok((_, None)) => {
                self.newer = true;
                None
            }
            Ok((saved_version, Some(prompt))) if prompt.version != saved_version => {
                self.load(&prompt, text, false)
            }
            Ok((_, Some(prompt))) if self.edit_revision == request.revision => {
                self.load(&prompt, text, true)
            }
            Ok((_, Some(prompt))) => {
                self.saved = prompt.text;
                self.base_version = prompt.version;
                self.newer = false;
                self.loaded = true;
                None
            }
            Err(error) => {
                self.error = Some(format!("Couldn't save the global prompt: {error}"));
                None
            }
        }
    }

    pub fn status(&self, text: &str) -> String {
        if !self.loaded {
            "Reading the global prompt…".to_owned()
        } else if self.pending {
            "Saving to every engine…".to_owned()
        } else if self.newer {
            "A newer version was saved elsewhere. Load it to continue from there.".to_owned()
        } else if text != self.saved {
            "Unsaved changes".to_owned()
        } else if self.base_version == 0 {
            "Not saved yet".to_owned()
        } else {
            format!("Saved · version {}", self.base_version)
        }
    }
}

use artisan_api::{BlobHash, EngineConfig, ProjectId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DraftBody {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<BlobHash>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SavedFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<EngineConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub follow_ups: Vec<Self>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedFile {
    pub path: DraftPath,
    #[serde(default)]
    pub folder: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DraftPath(String);

#[derive(Debug, thiserror::Error)]
#[error("{} is not a Unicode path, so it cannot be linked in a message", .path.display())]
pub struct NonUnicodePath {
    path: PathBuf,
}

impl DraftPath {
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<PathBuf> for DraftPath {
    type Error = NonUnicodePath;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        match path.into_os_string().into_string() {
            Ok(path) => Ok(Self(path)),
            Err(path) => Err(NonUnicodePath { path: path.into() }),
        }
    }
}

#[derive(Debug)]
pub(crate) enum RemoteBody {
    Readable(Box<DraftBody>),
    Unreadable(serde_json::Error),
}

pub(crate) static EMPTY_BODY: DraftBody = DraftBody {
    text: String::new(),
    images: Vec::new(),
    files: Vec::new(),
    config: None,
    project: None,
    follow_ups: Vec::new(),
};

impl DraftBody {
    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
            && self.images.is_empty()
            && self.files.is_empty()
            && self.config.is_none()
            && self.project.is_none()
            && self.follow_ups.is_empty()
    }

    pub(crate) fn to_wire(&self) -> Value {
        if self.is_empty() {
            Value::Null
        } else {
            serde_json::to_value(self).expect("invariant: a draft body serializes to JSON")
        }
    }

    pub(crate) fn add_image(&mut self, hash: BlobHash) {
        if !self.images.contains(&hash) {
            self.images.push(hash);
        }
    }
}

impl RemoteBody {
    pub(crate) fn from_wire(body: Value) -> Self {
        match body {
            Value::Null => Self::Readable(Box::default()),
            body @ (Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Array(_)
            | Value::Object(_)) => match serde_json::from_value(body) {
                Ok(body) => Self::Readable(Box::new(body)),
                Err(error) => Self::Unreadable(error),
            },
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::Readable(body) => body.is_empty(),
            Self::Unreadable(_) => false,
        }
    }

    pub(crate) fn matches(&self, local: &DraftBody) -> bool {
        match self {
            Self::Readable(body) => **body == *local,
            Self::Unreadable(_) => false,
        }
    }

    pub(crate) const fn unreadable(&self) -> Option<&serde_json::Error> {
        match self {
            Self::Readable(_) => None,
            Self::Unreadable(error) => Some(error),
        }
    }

    pub(crate) fn shown(&self) -> DraftBody {
        match self {
            Self::Readable(body) => (**body).clone(),
            Self::Unreadable(_) => DraftBody::default(),
        }
    }
}

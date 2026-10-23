//! The draft owns each conversion and upload until its blob is ready.
use artisan_api::BlobHash;
pub enum Input {
    Encoded,
    Uploaded(BlobHash),
    Failed(String),
}
pub enum State {
    Encoding,
    Uploading,
    Ready(BlobHash),
    Failed(String),
}
impl State {
    pub(crate) const fn hash(&self) -> Option<&BlobHash> {
        match self {
            Self::Ready(hash) => Some(hash),
            Self::Encoding | Self::Uploading | Self::Failed(_) => None,
        }
    }
    pub(crate) fn failure(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            Self::Encoding | Self::Uploading | Self::Ready(_) => None,
        }
    }
    /// Returns true only for the transition that starts the single upload.
    pub(crate) fn accept(&mut self, input: &Input) -> bool {
        match (&self, input) {
            (Self::Encoding, Input::Encoded) => {
                *self = Self::Uploading;
                true
            }
            (Self::Uploading, Input::Uploaded(hash)) => {
                *self = Self::Ready(hash.clone());
                false
            }
            (Self::Encoding | Self::Uploading, Input::Failed(reason)) => {
                *self = Self::Failed(reason.clone());
                false
            }
            _ => false,
        }
    }
}

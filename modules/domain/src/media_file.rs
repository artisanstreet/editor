//! Media files a reply names, read from the machine the Forge runs on.
//!
//! Image, audio, and model blocks name files on the agent's machine: the
//! Forge's.
//! The Editor parses the reply's `src` (URL, `data:`, `file://`, `~`) and
//! asks the Forge for the file by a clean path and the folder it is
//! relative to; the Forge resolves it by its own operating system's rules,
//! so a path means what the agent meant wherever the Editor runs.

use std::fmt;

use thiserror::Error;

use crate::{ROOT_PATH_MAX_BYTES, ThreadId};

/// Maximum size of one media file read from the Forge; one response must fit
/// a single transport frame with room to spare.
pub const MEDIA_FILE_MAX_BYTES: usize = 12 * 1024 * 1024;

/// The folder a relative [`MediaFilePath`] resolves against.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MediaPathAnchor {
    /// The thread's project folder; an absolute path stands on its own.
    Project,
    /// The Forge user's home folder (`~/…` in the reply).
    Home,
}

/// A media file path as the reply named it, before the Forge resolves it.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct MediaFilePath {
    text: String,
    anchor: MediaPathAnchor,
}

/// Why a media file path was refused before any file access.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum MediaFilePathError {
    /// A project-anchored path was empty.
    #[error("media file path is empty")]
    Empty,
    /// The path exceeds [`ROOT_PATH_MAX_BYTES`].
    #[error("media file path is longer than {ROOT_PATH_MAX_BYTES} bytes")]
    TooLong,
    /// The path contains a NUL byte, which no operating system accepts.
    #[error("media file path contains a NUL byte")]
    Nul,
}

impl MediaFilePath {
    /// Validates a path relative to `anchor`. A home-anchored path may be
    /// empty: it names the home folder itself.
    ///
    /// # Errors
    ///
    /// Returns [`MediaFilePathError`] for an empty project path, an overlong
    /// path, or one containing NUL.
    pub fn new(text: String, anchor: MediaPathAnchor) -> Result<Self, MediaFilePathError> {
        if text.is_empty() && anchor == MediaPathAnchor::Project {
            return Err(MediaFilePathError::Empty);
        }
        if text.len() > ROOT_PATH_MAX_BYTES {
            return Err(MediaFilePathError::TooLong);
        }
        if text.contains('\0') {
            return Err(MediaFilePathError::Nul);
        }
        Ok(Self { text, anchor })
    }

    /// The path text, relative to the anchor unless absolute.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The folder a relative path resolves against.
    #[must_use]
    pub const fn anchor(&self) -> MediaPathAnchor {
        self.anchor
    }
}

impl fmt::Debug for MediaFilePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaFilePath")
            .field("anchor", &self.anchor)
            .field("text_len", &self.text.len())
            .finish()
    }
}

/// Reads one image or audio file a reply in `thread_id` names, from the
/// Forge's machine. Relative paths resolve against the thread's project.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadMediaFile {
    thread_id: ThreadId,
    path: MediaFilePath,
}

impl ReadMediaFile {
    /// Constructs the read for one thread's media file.
    #[must_use]
    pub const fn new(thread_id: ThreadId, path: MediaFilePath) -> Self {
        Self { thread_id, path }
    }

    /// The thread whose project relative paths resolve against.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// The path as the reply named it.
    #[must_use]
    pub const fn path(&self) -> &MediaFilePath {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::{MediaFilePath, MediaFilePathError, MediaPathAnchor};
    use crate::ROOT_PATH_MAX_BYTES;

    #[test]
    fn paths_are_bounded_and_nul_free() {
        assert_eq!(
            MediaFilePath::new(String::new(), MediaPathAnchor::Project),
            Err(MediaFilePathError::Empty)
        );
        assert!(MediaFilePath::new(String::new(), MediaPathAnchor::Home).is_ok());
        assert_eq!(
            MediaFilePath::new("a\0b".into(), MediaPathAnchor::Project),
            Err(MediaFilePathError::Nul)
        );
        assert_eq!(
            MediaFilePath::new(
                "x".repeat(ROOT_PATH_MAX_BYTES + 1),
                MediaPathAnchor::Project
            ),
            Err(MediaFilePathError::TooLong)
        );
        let path = MediaFilePath::new("docs/a.png".into(), MediaPathAnchor::Project).unwrap();
        assert_eq!(path.text(), "docs/a.png");
        assert_eq!(path.anchor(), MediaPathAnchor::Project);
    }
}

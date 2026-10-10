//! Answers a reply's media-file read with the file's bytes from this
//! machine; see [`crate::media_file_service`].

use artisan_domain::{ReadMediaFile, RequestId};
use artisan_protocol::{
    ErrorCode, MediaFileResult, ProtocolFailure, ResponsePayload, ServerResponse,
};

use crate::media_file_service::{self, MediaFileError};

use super::RequestHandler;
use super::failures::{outcome, repository_failure, typed_failure};

impl RequestHandler {
    /// Resolves the path against the thread's project or this user's home
    /// folder and reads it on the blocking pool: a slow disk must not stall
    /// the event loop every connection shares.
    pub(super) async fn media_file_outcome(
        &self,
        read: &ReadMediaFile,
        request_id: &RequestId,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let root = self
            .repository
            .read_thread_project_root(read.thread_id())
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        let path = read.path().clone();
        let bytes = tokio::task::spawn_blocking(move || {
            let home = std::env::home_dir();
            let resolved = media_file_service::resolve(
                &path,
                std::path::Path::new(root.as_str()),
                home.as_deref(),
            )?;
            media_file_service::read(&resolved)
        })
        .await
        .map_err(|_| MediaFileError::Unreadable)
        .and_then(|read| read)
        .map_err(|error| {
            typed_failure(ErrorCode::InvalidInput, error.detail(), false, request_id)
        })?;
        Ok(outcome(
            request_id,
            ResponsePayload::MediaFile(MediaFileResult { bytes }),
        ))
    }
}

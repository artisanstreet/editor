//! Project attachment through a one-use directory selection.

use std::time::Instant;

use artisan_database::AttachProjectInput;
use artisan_domain::{AttachProject, ProjectId, RequestId};
use artisan_protocol::{ProtocolFailure, ResponsePayload, ServerResponse};

use super::failures::{outcome, repository_failure, unknown_directory_failure};
use super::{
    RequestHandler, forged_identity_failure, origin_clock_failure, origin_entropy_failure,
};

impl RequestHandler {
    /// Answers one attach mutation from its durable receipt or by consuming
    /// the one-use directory selection it names.
    pub(super) async fn attach_project_outcome(
        &self,
        request_id: &RequestId,
        attach: &AttachProject,
    ) -> Result<ServerResponse, ProtocolFailure> {
        if let Some(replay) = self
            .repository
            .lookup_attach_project(&attach.request_id, &attach.directory_id)
            .await
            .map_err(|error| repository_failure(&error, request_id))?
        {
            return Ok(outcome(
                request_id,
                ResponsePayload::AttachedProject {
                    project: replay.project,
                    disposition: replay.receipt.disposition,
                },
            ));
        }
        let Some(picker) = self.directory_picker.as_ref() else {
            return Err(unknown_directory_failure(request_id, &attach.directory_id));
        };
        let mut authority = picker.authority.lock().await;
        if let Some(replay) = self
            .repository
            .lookup_attach_project(&attach.request_id, &attach.directory_id)
            .await
            .map_err(|error| repository_failure(&error, request_id))?
        {
            return Ok(outcome(
                request_id,
                ResponsePayload::AttachedProject {
                    project: replay.project,
                    disposition: replay.receipt.disposition,
                },
            ));
        }
        let Some(selected) = authority.consume(&attach.directory_id, Instant::now()) else {
            return Err(unknown_directory_failure(request_id, &attach.directory_id));
        };
        let identity = self
            .origin
            .mint_identity()
            .map_err(|error| origin_entropy_failure(&error, request_id))?;
        let project_id = ProjectId::parse(identity)
            .map_err(|_| forged_identity_failure("project", request_id))?;
        let attached_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let result = self
            .repository
            .attach_project(AttachProjectInput {
                request_id: attach.request_id.clone(),
                directory_id: selected.directory_id,
                project_id,
                root_path: selected.root_path,
                display_name: selected.display_name,
                attached_at,
            })
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::AttachedProject {
                project: result.project,
                disposition: result.receipt.disposition,
            },
        ))
    }
}

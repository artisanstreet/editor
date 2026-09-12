//! Project-repository wire codec.
//!
//! Owns the project-repository query, entry, snapshot, branch, remote, and
//! host conversions plus their round-trip and malformed-input coverage.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn encode_project_repository_query(
    mut builder: artisan_capnp::project_repository_query::Builder<'_>,
    query: &ProjectRepositoryQuery,
) -> Result<(), ProtocolEncodeError> {
    let project_ids = query.project_ids();
    let mut encoded = builder.reborrow().init_project_ids(list_length(
        "request.queryProjectRepository.projectIds",
        project_ids.len(),
    )?);
    for (index, project_id) in project_ids.iter().enumerate() {
        encoded.set(
            list_index("request.queryProjectRepository.projectIds", index)?,
            project_id.as_str(),
        );
    }
    Ok(())
}

pub(crate) fn decode_project_repository_query(
    value: artisan_capnp::project_repository_query::Reader<'_>,
) -> Result<ProjectRepositoryQuery, ProtocolDecodeError> {
    let encoded = value.get_project_ids()?;
    let count = encoded.len() as usize;
    if count > PROJECT_REPOSITORY_MAXIMUM_PROJECTS {
        return Err(ProtocolDecodeError::ProtocolValue {
            source: ProtocolValueError::Repository {
                reason: "repository query names more projects than its bound",
            },
        });
    }
    let mut project_ids = Vec::with_capacity(count);
    for project_id in encoded {
        project_ids.push(parse_project_id(
            read_text(project_id, "request.queryProjectRepository.projectIds")?,
            "request.queryProjectRepository.projectIds",
        )?);
    }
    ProjectRepositoryQuery::new(project_ids)
        .map_err(|source| ProtocolDecodeError::ProtocolValue { source })
}

pub(crate) fn encode_project_repository_query_result(
    builder: artisan_capnp::project_repository_query_result::Builder<'_>,
    result: &ProjectRepositoryQueryResult,
) -> Result<(), ProtocolEncodeError> {
    let repositories = result.repositories();
    let mut encoded = builder.init_repositories(list_length(
        "response.projectRepository.repositories",
        repositories.len(),
    )?);
    for (index, entry) in repositories.iter().enumerate() {
        encode_project_repository_entry(
            encoded.reborrow().get(list_index(
                "response.projectRepository.repositories",
                index,
            )?),
            entry,
        )?;
    }
    Ok(())
}

pub(crate) fn decode_project_repository_query_result(
    value: artisan_capnp::project_repository_query_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let encoded = value.get_repositories()?;
    let count = encoded.len() as usize;
    if count > PROJECT_REPOSITORY_MAXIMUM_PROJECTS {
        return Err(ProtocolDecodeError::ProtocolValue {
            source: ProtocolValueError::Repository {
                reason: "repository result holds more projects than its bound",
            },
        });
    }
    let mut repositories = Vec::with_capacity(count);
    for entry in encoded {
        repositories.push(ProjectRepositoryEntry::new(
            parse_project_id(
                read_text(
                    entry.get_project_id(),
                    "response.projectRepository.projectId",
                )?,
                "response.projectRepository.projectId",
            )?,
            decode_project_repository(entry.get_repository()?)?,
        ));
    }
    Ok(ResponsePayload::ProjectRepository(
        ProjectRepositoryQueryResult::new(repositories)
            .map_err(|source| ProtocolDecodeError::ProtocolValue { source })?,
    ))
}

pub(crate) fn encode_project_repository_entry(
    mut builder: artisan_capnp::project_repository_entry::Builder<'_>,
    entry: &ProjectRepositoryEntry,
) -> Result<(), ProtocolEncodeError> {
    builder.set_project_id(entry.project_id().as_str());
    encode_project_repository(builder.init_repository(), entry.repository())
}

pub(crate) fn encode_project_repository(
    mut builder: artisan_capnp::project_repository::Builder<'_>,
    repository: &ProjectRepository,
) -> Result<(), ProtocolEncodeError> {
    match repository {
        ProjectRepository::NotRepository => {
            builder.set_state(artisan_capnp::ProjectRepositoryState::NotRepository);
        }
        ProjectRepository::Repository(snapshot) => {
            builder.set_state(artisan_capnp::ProjectRepositoryState::Repository);
            let mut encoded = builder.reborrow().init_snapshot();
            encode_repository_branch(encoded.reborrow().init_branch(), snapshot.branch());
            encoded.set_default_remote(snapshot.default_remote().unwrap_or(""));
            let remotes = snapshot.remotes();
            let mut list = encoded.init_remotes(list_length(
                "response.projectRepository.remotes",
                remotes.len(),
            )?);
            for (index, remote) in remotes.iter().enumerate() {
                encode_repository_remote(
                    list.reborrow()
                        .get(list_index("response.projectRepository.remotes", index)?),
                    remote,
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn encode_repository_branch(
    mut builder: artisan_capnp::repository_branch::Builder<'_>,
    branch: &RepositoryBranchState,
) {
    match branch {
        RepositoryBranchState::Attached { name } => {
            builder.set_kind(artisan_capnp::RepositoryBranchKind::Attached);
            builder.set_name(name);
        }
        RepositoryBranchState::Detached => {
            builder.set_kind(artisan_capnp::RepositoryBranchKind::Detached);
        }
        RepositoryBranchState::Unborn { name } => {
            builder.set_kind(artisan_capnp::RepositoryBranchKind::Unborn);
            builder.set_name(name);
        }
    }
}

pub(crate) fn encode_repository_remote(
    mut builder: artisan_capnp::repository_remote::Builder<'_>,
    remote: &RepositoryRemote,
) {
    builder.set_host(encode_repository_host(remote.host()));
    builder.set_name(remote.name());
    builder.set_url(remote.url());
    builder.set_web_url(remote.web_url().unwrap_or(""));
}

pub(crate) fn encode_repository_host(host: RepositoryHost) -> artisan_capnp::RepositoryHost {
    match host {
        RepositoryHost::Azure => artisan_capnp::RepositoryHost::Azure,
        RepositoryHost::Bitbucket => artisan_capnp::RepositoryHost::Bitbucket,
        RepositoryHost::Codeberg => artisan_capnp::RepositoryHost::Codeberg,
        RepositoryHost::Gitea => artisan_capnp::RepositoryHost::Gitea,
        RepositoryHost::GitHub => artisan_capnp::RepositoryHost::Github,
        RepositoryHost::GitLab => artisan_capnp::RepositoryHost::Gitlab,
        RepositoryHost::Other => artisan_capnp::RepositoryHost::Other,
        RepositoryHost::Sourcehut => artisan_capnp::RepositoryHost::Sourcehut,
        RepositoryHost::Unknown => artisan_capnp::RepositoryHost::Unknown,
    }
}

pub(crate) fn decode_repository_host(host: artisan_capnp::RepositoryHost) -> RepositoryHost {
    match host {
        artisan_capnp::RepositoryHost::Azure => RepositoryHost::Azure,
        artisan_capnp::RepositoryHost::Bitbucket => RepositoryHost::Bitbucket,
        artisan_capnp::RepositoryHost::Codeberg => RepositoryHost::Codeberg,
        artisan_capnp::RepositoryHost::Gitea => RepositoryHost::Gitea,
        artisan_capnp::RepositoryHost::Github => RepositoryHost::GitHub,
        artisan_capnp::RepositoryHost::Gitlab => RepositoryHost::GitLab,
        artisan_capnp::RepositoryHost::Other => RepositoryHost::Other,
        artisan_capnp::RepositoryHost::Sourcehut => RepositoryHost::Sourcehut,
        artisan_capnp::RepositoryHost::Unknown => RepositoryHost::Unknown,
    }
}

pub(crate) fn decode_project_repository(
    value: artisan_capnp::project_repository::Reader<'_>,
) -> Result<ProjectRepository, ProtocolDecodeError> {
    match value.get_state()? {
        artisan_capnp::ProjectRepositoryState::NotRepository => {
            Ok(ProjectRepository::NotRepository)
        }
        artisan_capnp::ProjectRepositoryState::Repository => {
            let snapshot = value.get_snapshot()?;
            let branch = decode_repository_branch(snapshot.get_branch()?)?;
            let default_remote = match read_text(
                snapshot.get_default_remote(),
                "response.projectRepository.defaultRemote",
            )? {
                name if name.is_empty() => None,
                name => Some(name),
            };
            let encoded = snapshot.get_remotes()?;
            let count = encoded.len() as usize;
            if count > REPOSITORY_REMOTE_MAXIMUM {
                return Err(ProtocolDecodeError::ProtocolValue {
                    source: ProtocolValueError::Repository {
                        reason: "repository holds more remotes than its bound",
                    },
                });
            }
            let mut remotes = Vec::with_capacity(count);
            for remote in encoded {
                let web_url =
                    match read_text(remote.get_web_url(), "response.projectRepository.webUrl")? {
                        url if url.is_empty() => None,
                        url => Some(url),
                    };
                remotes.push(
                    RepositoryRemote::new(
                        decode_repository_host(remote.get_host()?),
                        read_text(remote.get_name(), "response.projectRepository.name")?,
                        read_text(remote.get_url(), "response.projectRepository.url")?,
                        web_url,
                    )
                    .map_err(|source| ProtocolDecodeError::ProtocolValue { source })?,
                );
            }
            let snapshot = RepositorySnapshot::new(branch, default_remote, remotes)
                .map_err(|source| ProtocolDecodeError::ProtocolValue { source })?;
            Ok(ProjectRepository::Repository(snapshot))
        }
    }
}

pub(crate) fn decode_repository_branch(
    value: artisan_capnp::repository_branch::Reader<'_>,
) -> Result<RepositoryBranchState, ProtocolDecodeError> {
    let branch = match value.get_kind()? {
        artisan_capnp::RepositoryBranchKind::Attached => RepositoryBranchState::attached(
            read_text(value.get_name(), "response.projectRepository.branch.name")?,
        ),
        artisan_capnp::RepositoryBranchKind::Detached => Ok(RepositoryBranchState::detached()),
        artisan_capnp::RepositoryBranchKind::Unborn => RepositoryBranchState::unborn(read_text(
            value.get_name(),
            "response.projectRepository.branch.name",
        )?),
    };
    branch.map_err(|source| ProtocolDecodeError::ProtocolValue { source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProtocolVersion;

    fn envelope(body: WireEnvelopeBody) -> WireEnvelope {
        WireEnvelope {
            protocol_version: ProtocolVersion::V1,
            frame_id: FrameId::parse("frame-1").expect("frame id is valid"),
            sent_at: UnixMillis::from_millis(1_700_000_000_000),
            body,
        }
    }

    fn repository_snapshot() -> RepositorySnapshot {
        RepositorySnapshot::new(
            RepositoryBranchState::attached("main").expect("branch"),
            Some("origin".to_owned()),
            vec![
                RepositoryRemote::new(
                    RepositoryHost::GitHub,
                    "origin",
                    "git@github.com:artisanstreet/editor.git",
                    Some("https://github.com/artisanstreet/editor".to_owned()),
                )
                .expect("github remote"),
                RepositoryRemote::new(
                    RepositoryHost::Unknown,
                    "backup",
                    "/srv/backup/editor.git",
                    None,
                )
                .expect("local remote"),
            ],
        )
        .expect("snapshot")
    }

    #[test]
    fn project_repository_query_request_round_trips() {
        let query = ProjectRepositoryQuery::new(vec![
            ProjectId::parse("project-1").expect("project"),
            ProjectId::parse("project-2").expect("project"),
        ])
        .expect("query");
        let wire = envelope(WireEnvelopeBody::Request(
            ClientRequest::QueryProjectRepository(query),
        ));
        let encoded = encode_envelope(&wire).expect("request encodes");
        let decoded = decode_envelope(&encoded).expect("request decodes");
        assert!(decoded.body == wire.body);
    }

    #[test]
    fn project_repository_response_round_trips_every_observation() {
        let result = ProjectRepositoryQueryResult::new(vec![
            ProjectRepositoryEntry::new(
                ProjectId::parse("project-1").expect("project"),
                ProjectRepository::Repository(repository_snapshot()),
            ),
            ProjectRepositoryEntry::new(
                ProjectId::parse("project-2").expect("project"),
                ProjectRepository::NotRepository,
            ),
        ])
        .expect("result");
        let wire = envelope(WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse("frame-1").expect("request id is valid"),
            payload: ResponsePayload::ProjectRepository(result),
        }));
        let encoded = encode_envelope(&wire).expect("response encodes");
        let decoded = decode_envelope(&encoded).expect("response decodes");
        assert!(decoded.body == wire.body);
    }

    #[test]
    fn project_repository_response_preserves_unborn_and_detached_branches() {
        for branch in [
            RepositoryBranchState::unborn("main").expect("branch"),
            RepositoryBranchState::detached(),
        ] {
            let snapshot = RepositorySnapshot::new(branch, None, vec![]).expect("snapshot");
            let result = ProjectRepositoryQueryResult::new(vec![ProjectRepositoryEntry::new(
                ProjectId::parse("project-1").expect("project"),
                ProjectRepository::Repository(snapshot),
            )])
            .expect("result");
            let wire = envelope(WireEnvelopeBody::Response(ServerResponse {
                request_id: RequestId::parse("frame-1").expect("request id is valid"),
                payload: ResponsePayload::ProjectRepository(result),
            }));
            let encoded = encode_envelope(&wire).expect("response encodes");
            let decoded = decode_envelope(&encoded).expect("response decodes");
            assert!(decoded.body == wire.body);
        }
    }

    #[test]
    fn decode_rejects_repository_with_remotes_but_no_default() {
        let mut message = Builder::new(HeapAllocator::new());
        {
            let mut root = message.init_root::<artisan_capnp::envelope::Builder>();
            root.set_protocol_version(1);
            root.set_message_id("frame-1");
            root.set_sent_at_millis(0);
            let mut response = root.reborrow().init_body().init_response();
            response.set_request_id("frame-1");
            let result = response.init_project_repository();
            let mut entries = result.init_repositories(1);
            let mut entry = entries.reborrow().get(0);
            entry.set_project_id("project-1");
            let mut repository = entry.init_repository();
            repository.set_state(artisan_capnp::ProjectRepositoryState::Repository);
            let mut snapshot = repository.reborrow().init_snapshot();
            snapshot
                .reborrow()
                .init_branch()
                .set_kind(artisan_capnp::RepositoryBranchKind::Detached);
            snapshot.set_default_remote("");
            let mut remotes = snapshot.init_remotes(1);
            let mut remote = remotes.reborrow().get(0);
            remote.set_host(artisan_capnp::RepositoryHost::Github);
            remote.set_name("origin");
            remote.set_url("https://github.com/artisanstreet/editor.git");
            remote.set_web_url("https://github.com/artisanstreet/editor");
        }
        let bytes = serialize::write_message_to_words(&message);
        assert!(matches!(
            decode_envelope(&bytes),
            Err(ProtocolDecodeError::ProtocolValue { .. })
        ));
    }
}

#[allow(dead_code)]
#[path = "../../modules/backend/src/preview_service_policy.rs"]
mod preview_service_policy;

use preview_service_policy::{
    PreviewDispatchLease, PreviewDispatchLeaseInput, PreviewDispatchLeaseKind,
    PreviewInspectionAction, PreviewInspectionCommand, PreviewInspectionReconnectState,
    PreviewRepositoryResultKind, PreviewServiceAction, PreviewServiceError, PreviewServicePolicy,
    PreviewSource, PreviewTargetLaunchState, PreviewTargetState, PreviewTargetUpdateAction,
    PreviewTargetUpdateCommand, get, list, recover_dispatch_leases, recover_inspections,
    validate_preview_target_id, validate_target_url,
};

fn update_command(action: PreviewTargetUpdateAction) -> PreviewTargetUpdateCommand {
    PreviewTargetUpdateCommand {
        action,
        health_json: Some("{\"ok\":true}".to_owned()),
        last_error: None,
        launch_state: Some(PreviewTargetLaunchState::Launched),
        message_id: "message-1".to_owned(),
        state: Some(PreviewTargetState::Healthy),
        target_id: "target-1".to_owned(),
        thread_id: "thread-1".to_owned(),
    }
}

fn inspection_command() -> PreviewInspectionCommand {
    PreviewInspectionCommand {
        action: PreviewInspectionAction::Open,
        connector_id: Some("connector-1".to_owned()),
        last_error: None,
        message_id: "message-2".to_owned(),
        reconnect_state: Some(PreviewInspectionReconnectState::Connected),
        session_id: "session-1".to_owned(),
        target_id: Some("target-1".to_owned()),
        thread_id: "thread-1".to_owned(),
    }
}

fn lease() -> PreviewDispatchLease {
    PreviewDispatchLease {
        acquired_at: "2026-08-28T10:00:00Z".to_owned(),
        expires_at: "2026-08-28T10:01:00Z".to_owned(),
        kind: PreviewDispatchLeaseKind::Launch,
        lease_id: "lease-1".to_owned(),
        owner_instance_id: "instance-1".to_owned(),
        session_id: None,
        target_id: Some("target-1".to_owned()),
        thread_id: "thread-1".to_owned(),
    }
}

#[test]
fn target_ids_use_unicode_scalar_boundaries() {
    assert_eq!(
        validate_preview_target_id(""),
        Err(PreviewServiceError::InvalidTargetId)
    );
    assert!(validate_preview_target_id("a").is_ok());

    let one_scalar_emoji = "🦀";
    assert_eq!(one_scalar_emoji.chars().count(), 1);
    assert!(validate_preview_target_id(one_scalar_emoji).is_ok());

    let two_hundred_fifty_six_scalars = "é".repeat(256);
    assert_eq!(two_hundred_fifty_six_scalars.chars().count(), 256);
    assert!(validate_preview_target_id(&two_hundred_fifty_six_scalars).is_ok());

    let two_hundred_fifty_seven_scalars = "é".repeat(257);
    assert_eq!(two_hundred_fifty_seven_scalars.chars().count(), 257);
    let error = validate_preview_target_id(&two_hundred_fifty_seven_scalars).unwrap_err();
    assert_eq!(error, PreviewServiceError::InvalidTargetId);
    assert!(!format!("{error:?}").contains(&two_hundred_fifty_seven_scalars));
}

#[test]
fn get_suppresses_invalid_repository_call_and_forwards_valid_id() {
    assert_eq!(get(""), Err(PreviewServiceError::InvalidTargetId));

    let target_id = "target-🦀";
    assert_eq!(
        get(target_id),
        Ok(PreviewServiceAction::GetTarget {
            target_id: target_id.to_owned(),
        })
    );
}

#[test]
fn list_forwards_optional_workspace() {
    assert_eq!(
        list(None),
        PreviewServiceAction::ListTargets { workspace_id: None }
    );
    assert_eq!(
        list(Some("workspace-1")),
        PreviewServiceAction::ListTargets {
            workspace_id: Some("workspace-1".to_owned())
        }
    );
}

#[test]
fn forwarding_preserves_url_register_and_update_inputs() {
    assert_eq!(
        validate_target_url("http://preview.invalid:4312"),
        PreviewServiceAction::ValidateLocalPreviewUrl {
            url: "http://preview.invalid:4312".to_owned()
        }
    );

    let register_input = preview_service_policy::PreviewRegisterCommand {
        message_id: "message-3".to_owned(),
        port: 4312,
        project_id: "project-1".to_owned(),
        routes: Some(vec!["/".to_owned(), "/health".to_owned()]),
        source: Some(PreviewSource::Process {
            process_id: "process-1".to_owned(),
        }),
        target_id: "target-1".to_owned(),
        thread_id: "thread-1".to_owned(),
        url: "http://127.0.0.1:4312".to_owned(),
        workspace_id: "workspace-1".to_owned(),
    };
    assert_eq!(
        PreviewServicePolicy::register(register_input.clone()),
        PreviewServiceAction::Register {
            input: register_input
        }
    );

    let replay = PreviewServicePolicy::replay_target_update(update_command(
        PreviewTargetUpdateAction::Probe,
    ));
    let update = PreviewServicePolicy::update_target(
        update_command(PreviewTargetUpdateAction::Probe),
        Some("lease-1"),
    );
    assert!(matches!(
        replay,
        PreviewServiceAction::ReplayTargetUpdate { .. }
    ));
    assert!(matches!(update, PreviewServiceAction::UpdateTarget { .. }));
    assert_ne!(replay, update);
    assert_eq!(
        replay.result_kind(),
        PreviewRepositoryResultKind::OptionalTarget
    );
    assert_eq!(update.result_kind(), PreviewRepositoryResultKind::Target);

    match update {
        PreviewServiceAction::UpdateTarget {
            dispatch_lease_id, ..
        } => assert_eq!(dispatch_lease_id.as_deref(), Some("lease-1")),
        _ => unreachable!(),
    }
}

#[test]
fn inspection_and_dispatch_lease_forwarding_keep_optional_values() {
    let inspection_input = inspection_command();
    let inspection =
        PreviewServicePolicy::update_inspection(inspection_input.clone(), Some("lease-2"));
    assert_eq!(
        inspection,
        PreviewServiceAction::UpdateInspection {
            input: inspection_input,
            dispatch_lease_id: Some("lease-2".to_owned()),
        }
    );
    assert_eq!(
        PreviewServicePolicy::update_inspection(inspection_command(), None),
        PreviewServiceAction::UpdateInspection {
            input: inspection_command(),
            dispatch_lease_id: None,
        }
    );

    let lease_input = PreviewDispatchLeaseInput {
        kind: PreviewDispatchLeaseKind::InspectionHealth,
        session_id: Some("session-1".to_owned()),
        target_id: None,
        thread_id: "thread-1".to_owned(),
    };
    assert_eq!(
        PreviewServicePolicy::acquire_dispatch_lease(lease_input.clone()),
        PreviewServiceAction::AcquireDispatchLease { input: lease_input }
    );

    let lease = lease();
    assert_eq!(
        PreviewServicePolicy::release_dispatch_lease(lease.clone()),
        PreviewServiceAction::ReleaseDispatchLease {
            lease: lease.clone()
        }
    );
    assert_eq!(
        PreviewServicePolicy::renew_dispatch_lease(lease.clone()),
        PreviewServiceAction::RenewDispatchLease { lease }
    );
}

#[test]
fn all_service_commands_have_distinct_typed_actions() {
    let actions = [
        get("target-1").unwrap(),
        PreviewServicePolicy::list(None),
        PreviewServicePolicy::validate_target_url("https://localhost:4312"),
        PreviewServicePolicy::register(preview_service_policy::PreviewRegisterCommand {
            message_id: "message-4".to_owned(),
            port: 4312,
            project_id: "project-1".to_owned(),
            routes: None,
            source: None,
            target_id: "target-1".to_owned(),
            thread_id: "thread-1".to_owned(),
            url: "https://localhost:4312".to_owned(),
            workspace_id: "workspace-1".to_owned(),
        }),
        PreviewServicePolicy::replay_target_update(update_command(
            PreviewTargetUpdateAction::Launch,
        )),
        PreviewServicePolicy::update_target(
            update_command(PreviewTargetUpdateAction::Remove),
            None,
        ),
        PreviewServicePolicy::update_inspection(inspection_command(), None),
        PreviewServicePolicy::recover_inspections(),
        PreviewServicePolicy::acquire_dispatch_lease(PreviewDispatchLeaseInput {
            kind: PreviewDispatchLeaseKind::Probe,
            session_id: None,
            target_id: Some("target-1".to_owned()),
            thread_id: "thread-1".to_owned(),
        }),
        PreviewServicePolicy::release_dispatch_lease(lease()),
        PreviewServicePolicy::renew_dispatch_lease(lease()),
        PreviewServicePolicy::recover_dispatch_leases(),
    ];

    assert!(matches!(actions[0], PreviewServiceAction::GetTarget { .. }));
    assert!(matches!(
        actions[1],
        PreviewServiceAction::ListTargets { .. }
    ));
    assert!(matches!(
        actions[2],
        PreviewServiceAction::ValidateLocalPreviewUrl { .. }
    ));
    assert!(matches!(actions[3], PreviewServiceAction::Register { .. }));
    assert!(matches!(
        actions[4],
        PreviewServiceAction::ReplayTargetUpdate { .. }
    ));
    assert!(matches!(
        actions[5],
        PreviewServiceAction::UpdateTarget { .. }
    ));
    assert!(matches!(
        actions[6],
        PreviewServiceAction::UpdateInspection { .. }
    ));
    assert!(matches!(
        actions[7],
        PreviewServiceAction::RecoverInspections
    ));
    assert!(matches!(
        actions[8],
        PreviewServiceAction::AcquireDispatchLease { .. }
    ));
    assert!(matches!(
        actions[9],
        PreviewServiceAction::ReleaseDispatchLease { .. }
    ));
    assert!(matches!(
        actions[10],
        PreviewServiceAction::RenewDispatchLease { .. }
    ));
    assert!(matches!(
        actions[11],
        PreviewServiceAction::RecoverDispatchLeases
    ));

    assert_eq!(
        recover_inspections().result_kind(),
        PreviewRepositoryResultKind::Inspections
    );
    assert_eq!(
        recover_dispatch_leases().result_kind(),
        PreviewRepositoryResultKind::DispatchLeases
    );
}

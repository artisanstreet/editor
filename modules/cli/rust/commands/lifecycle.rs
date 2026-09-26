use std::{
    net::SocketAddr,
    num::NonZeroU32,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use artisan_domain::{RequestId, UnixMillis};
use artisan_protocol::{
    ClientRequest, ErrorCode, FrameId, Hello, HelloCredential, LifecycleRequest, LifecycleResponse,
    LifecycleState, LifecycleStatus, LifecycleStopDisposition, LifecycleStopReceipt,
    ProtocolVersion, ResponsePayload, VersionOffer, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, ClientRequestError, ClientSession, ClientSessionError, ClientSessionLimits,
    LoopbackTarget, PinnedIdentity, RequestOutcome,
};
use rustls_pki_types::CertificateDer;

use crate::{
    CliError, Result,
    credentials::{
        self, ForgeCredentialError, ReconnectAttempt, ReconnectBinding, ReconnectCapabilityStore,
    },
    instance::NativeInstanceConfig,
    paths::Layout,
    process,
};

use super::{load_native_instance, require_launchable_installation};

static NEXT_LIFECYCLE_FRAME: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LifecycleOperation {
    Status,
    Stop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum LifecycleResult {
    Status(LifecycleStatus),
    Stop(LifecycleStopReceipt),
}

struct LifecycleMaterial {
    certificate: CertificateDer<'static>,
    pinned_identity: PinnedIdentity,
    target: LoopbackTarget,
    binding: ReconnectBinding,
    limits: ClientSessionLimits,
}

pub(super) fn status(layout: &Layout, json: bool) -> Result<()> {
    let manifest = require_launchable_installation(layout)?;
    let config = load_lifecycle_instance(layout)?;
    match process::readiness_status(config.readiness_path(), &manifest.forge_executable()) {
        process::ForgeReadinessStatus::Ready(readiness) => {
            let result = match authenticated_lifecycle(
                layout,
                &config,
                &readiness,
                LifecycleOperation::Status,
            ) {
                // The Forge's only control session belongs to the Editor
                // it serves; readiness is all `ae` can report.
                Err(
                    CliError::LifecycleReadiness { .. } | CliError::LifecycleCredentialState { .. },
                ) => {
                    print_readiness_status(&readiness, json);
                    return Ok(());
                }
                result => result?,
            };
            let LifecycleResult::Status(lifecycle) = result else {
                return Err(CliError::LifecycleService {
                    reason: "unexpected lifecycle response",
                });
            };
            print_lifecycle_status(&readiness, &lifecycle, json);
            Ok(())
        }
        process::ForgeReadinessStatus::Missing => {
            if json {
                println!(r#"{{"readiness":"missing"}}"#);
            } else {
                println!("missing");
            }
            Ok(())
        }
        process::ForgeReadinessStatus::Invalid => {
            if json {
                println!(r#"{{"readiness":"invalid"}}"#);
            } else {
                println!("invalid");
            }
            Ok(())
        }
    }
}

pub(super) fn stop(layout: &Layout, pid: NonZeroU32, if_idle: bool) -> Result<()> {
    if !if_idle {
        return Err(CliError::Unsupported("stop requires --if-idle".to_owned()));
    }

    let manifest = require_launchable_installation(layout)?;
    let config = load_lifecycle_instance(layout)?;
    match process::readiness_status(config.readiness_path(), &manifest.forge_executable()) {
        process::ForgeReadinessStatus::Missing => Err(CliError::NotRunning),
        process::ForgeReadinessStatus::Invalid => Err(CliError::LifecycleReadiness {
            reason: "readiness receipt is invalid or stale",
        }),
        process::ForgeReadinessStatus::Ready(readiness) => {
            if readiness.pid() != pid.get() {
                return Err(CliError::LifecycleReadiness {
                    reason: "PID does not match the readiness receipt",
                });
            }
            if super::autostart::stop_service(layout, pid.get())? {
                println!("stopped the Forge service");
                return Ok(());
            }
            let result =
                authenticated_lifecycle(layout, &config, &readiness, LifecycleOperation::Stop)?;
            let LifecycleResult::Stop(receipt) = result else {
                return Err(CliError::LifecycleService {
                    reason: "unexpected lifecycle response",
                });
            };
            print_stop_receipt(&receipt);
            Ok(())
        }
    }
}

fn load_lifecycle_instance(layout: &Layout) -> Result<NativeInstanceConfig> {
    match load_native_instance(layout) {
        Ok(config) => Ok(config),
        Err(CliError::MissingInstance) => Err(CliError::MissingInstance),
        Err(_) => Err(CliError::LifecycleService {
            reason: "native instance configuration is unavailable",
        }),
    }
}

fn authenticated_lifecycle(
    layout: &Layout,
    config: &NativeInstanceConfig,
    readiness: &process::ForgeReadiness,
    operation: LifecycleOperation,
) -> Result<LifecycleResult> {
    let material = lifecycle_material(layout, config, readiness)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| CliError::LifecycleService {
            reason: "create lifecycle runtime",
        })?;
    let store = ReconnectCapabilityStore::from_home(&layout.root)
        .map_err(|error| lifecycle_credential_error(&error))?;
    let attempt = store
        .checkout(material.binding, credentials::RECONNECT_LOCK_TIMEOUT)
        .map_err(|error| lifecycle_credential_error(&error))?;
    runtime.block_on(authenticated_lifecycle_session(
        material, operation, attempt,
    ))
}

fn lifecycle_material(
    layout: &Layout,
    config: &NativeInstanceConfig,
    readiness: &process::ForgeReadiness,
) -> Result<LifecycleMaterial> {
    let identity = credentials::load_existing_client_identity(&layout.root)
        .map_err(|error| lifecycle_credential_error(&error))?;
    if config.credentials_manifest() != identity.paths().manifest_path() {
        return Err(CliError::LifecycleCredentialState {
            reason: "credential manifest does not match the instance",
        });
    }

    let certificate = identity.certificate().clone();
    let pinned_identity = PinnedIdentity::from_certificate(&certificate);
    let expected_pin = pinned_identity.to_hex();
    if readiness.certificate_sha256() != expected_pin
        || readiness.certificate_sha256() != readiness.certificate_sha256().to_ascii_lowercase()
    {
        return Err(CliError::LifecycleReadiness {
            reason: "readiness certificate does not match the client identity",
        });
    }

    let address =
        readiness
            .endpoint()
            .parse::<SocketAddr>()
            .map_err(|_| CliError::LifecycleReadiness {
                reason: "readiness endpoint is invalid",
            })?;
    let target = LoopbackTarget::new(address).map_err(|_| CliError::LifecycleReadiness {
        reason: "readiness endpoint is not exact loopback",
    })?;
    let pid = NonZeroU32::new(readiness.pid()).ok_or(CliError::LifecycleReadiness {
        reason: "readiness PID is zero",
    })?;
    let binding = ReconnectBinding::new(
        config.instance_id(),
        target.addr().port(),
        *pinned_identity.as_bytes(),
        pid,
    )
    .map_err(|error| lifecycle_credential_error(&error))?;
    let listener = config.listener();
    let limits = ClientSessionLimits {
        connect: lifecycle_duration(listener.admission_timeout_ms())?,
        handshake: lifecycle_duration(listener.handshake_timeout_ms())?,
        request: lifecycle_duration(listener.request_timeout_ms())?,
        shutdown: lifecycle_duration(listener.drain_timeout_ms())?,
        admission_budget: usize::try_from(listener.requests_per_connection().get()).map_err(
            |_| CliError::LifecycleService {
                reason: "request admission budget is not representable",
            },
        )?,
    };

    Ok(LifecycleMaterial {
        certificate,
        pinned_identity,
        target,
        binding,
        limits,
    })
}

fn lifecycle_duration(milliseconds: u64) -> Result<Duration> {
    if milliseconds == 0 {
        return Err(CliError::LifecycleService {
            reason: "listener timeout is zero",
        });
    }
    Ok(Duration::from_millis(milliseconds))
}

async fn authenticated_lifecycle_session(
    material: LifecycleMaterial,
    operation: LifecycleOperation,
    mut attempt: ReconnectAttempt,
) -> Result<LifecycleResult> {
    let cancel = CancelHandle::new();
    let capability = match attempt.take_credential() {
        Ok(capability) => capability,
        Err(error) => {
            let primary = lifecycle_credential_error(&error);
            return match attempt.quarantine() {
                Ok(()) => Err(primary),
                Err(custody) => Err(lifecycle_credential_error(&custody)),
            };
        }
    };
    let hello = match lifecycle_hello_with_capability(capability) {
        Ok(hello) => hello,
        Err((failure, capability)) => {
            return match attempt.restore_before_handshake(capability) {
                Ok(_) => Err(failure),
                Err(custody) => Err(lifecycle_credential_error(&custody)),
            };
        }
    };

    let connected = ClientSession::connect(
        material.target,
        material.certificate.clone(),
        material.pinned_identity,
        hello,
        material.limits,
        &cancel,
    )
    .await;
    let (session, welcome) = match connected {
        Ok(connected) => connected,
        Err(error) => {
            let failure = lifecycle_connect_error(&error);
            return match attempt.quarantine() {
                Ok(()) => Err(failure),
                Err(custody) => Err(lifecycle_credential_error(&custody)),
            };
        }
    };
    let reconnect_lease =
        match attempt.publish_next(material.binding, welcome.welcome.reconnect_capability) {
            Ok(lease) => lease,
            Err(error) => {
                let _ = session.shutdown(&cancel).await;
                return Err(lifecycle_credential_error(&error));
            }
        };

    if !session.lifecycle_control_supported() {
        let _ = session.shutdown(&cancel).await;
        drop(reconnect_lease);
        return Err(CliError::UnsupportedLifecycleControl);
    }

    let (request, expected_request_id) = match lifecycle_request(operation) {
        Ok(request) => request,
        Err(error) => {
            let _ = session.shutdown(&cancel).await;
            drop(reconnect_lease);
            return Err(error);
        }
    };
    let (session, resolved) = match session
        .request_acknowledging_response(request, &cancel)
        .await
    {
        Ok(response) => response,
        Err(error) => {
            let quarantine = lifecycle_request_requires_quarantine(&error);
            let failure = lifecycle_request_error(&error);
            if quarantine {
                return match reconnect_lease.quarantine() {
                    Ok(()) => Err(failure),
                    Err(custody) => Err(lifecycle_credential_error(&custody)),
                };
            }
            drop(reconnect_lease);
            return Err(failure);
        }
    };
    let result = classify_lifecycle_response(operation, &expected_request_id, &resolved);
    // The request stage has already settled its terminal outcome. Shutdown is
    // best-effort here; it cannot turn an acknowledged stop into a retryable
    // operation and the session is consumed even if the bounded drain fails.
    let _ = session.shutdown(&cancel).await;
    if lifecycle_response_requires_quarantine(&expected_request_id, &resolved, &result) {
        return match reconnect_lease.quarantine() {
            Ok(()) => result,
            Err(custody) => Err(lifecycle_credential_error(&custody)),
        };
    }
    drop(reconnect_lease);
    result
}

pub(super) fn lifecycle_hello_with_capability(
    capability: artisan_protocol::ReconnectCapability,
) -> std::result::Result<WireEnvelope, (CliError, artisan_protocol::ReconnectCapability)> {
    let (frame_id, sent_at) = match lifecycle_frame_stamp() {
        Ok(stamp) => stamp,
        Err(error) => return Err((error, capability)),
    };
    let Ok(supported_versions) = VersionOffer::new(vec![1]) else {
        return Err((
            CliError::LifecycleService {
                reason: "build protocol version offer",
            },
            capability,
        ));
    };
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id,
        sent_at,
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions,
            credential: HelloCredential::Reconnect(capability),
            supports_lifecycle_control: true,
        }),
    })
}

pub(super) fn lifecycle_request(
    operation: LifecycleOperation,
) -> Result<(WireEnvelope, RequestId)> {
    let (frame_id, sent_at) = lifecycle_frame_stamp()?;
    let request_id = frame_id
        .to_request_id()
        .map_err(|_| CliError::LifecycleService {
            reason: "build request correlation",
        })?;
    let request = match operation {
        LifecycleOperation::Status => LifecycleRequest::Status,
        LifecycleOperation::Stop => LifecycleRequest::Stop { require_idle: true },
    };
    let envelope = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id,
        sent_at,
        body: WireEnvelopeBody::Request(ClientRequest::Lifecycle(request)),
    };
    envelope
        .validate_correlation()
        .map_err(|_| CliError::LifecycleService {
            reason: "validate request correlation",
        })?;
    Ok((envelope, request_id))
}

fn lifecycle_frame_stamp() -> Result<(FrameId, UnixMillis)> {
    let sent_at = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            UnixMillis::from_millis(i64::try_from(duration.as_millis()).map_err(|_| {
                CliError::LifecycleService {
                    reason: "build frame timestamp",
                }
            })?)
        }
        Err(error) => UnixMillis::from_millis(
            i64::try_from(error.duration().as_millis())
                .map_err(|_| CliError::LifecycleService {
                    reason: "build frame timestamp",
                })?
                .saturating_neg(),
        ),
    };
    let sequence = NEXT_LIFECYCLE_FRAME
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| CliError::LifecycleService {
            reason: "lifecycle frame sequence exhausted",
        })?
        .checked_add(1)
        .ok_or(CliError::LifecycleService {
            reason: "lifecycle frame sequence exhausted",
        })?;
    let text = format!(
        "native-{}-{}-{}",
        std::process::id(),
        sent_at.as_millis(),
        sequence
    );
    let frame_id = FrameId::parse(text).map_err(|_| CliError::LifecycleService {
        reason: "build frame identity",
    })?;
    frame_id
        .to_request_id()
        .map_err(|_| CliError::LifecycleService {
            reason: "build frame correlation",
        })?;
    Ok((frame_id, sent_at))
}

fn classify_lifecycle_response(
    operation: LifecycleOperation,
    expected_request_id: &RequestId,
    resolved: &artisan_transport::ResolvedRequest,
) -> Result<LifecycleResult> {
    if resolved.request_id() != expected_request_id {
        return Err(CliError::LifecycleService {
            reason: "response correlation failed",
        });
    }
    match resolved.outcome() {
        RequestOutcome::Failure(failure) => classify_lifecycle_failure(operation, failure.code),
        RequestOutcome::Response(response) => match (&operation, &response.payload) {
            (
                LifecycleOperation::Status,
                ResponsePayload::Lifecycle(LifecycleResponse::Status(status)),
            ) => {
                if status.validate().is_err() {
                    return Err(CliError::LifecycleService {
                        reason: "lifecycle status was invalid",
                    });
                }
                Ok(LifecycleResult::Status(status.clone()))
            }
            (
                LifecycleOperation::Stop,
                ResponsePayload::Lifecycle(LifecycleResponse::Stop(receipt)),
            ) => {
                classify_stop_receipt(receipt)?;
                Ok(LifecycleResult::Stop(receipt.clone()))
            }
            _ => Err(CliError::LifecycleService {
                reason: "unexpected lifecycle response payload",
            }),
        },
    }
}

pub(super) fn classify_stop_receipt(receipt: &LifecycleStopReceipt) -> Result<()> {
    if receipt.state != LifecycleState::Draining {
        return Err(CliError::LifecycleService {
            reason: "stop response did not enter draining",
        });
    }
    Ok(())
}

pub(super) fn classify_lifecycle_failure(
    operation: LifecycleOperation,
    code: ErrorCode,
) -> Result<LifecycleResult> {
    match code {
        ErrorCode::UnsupportedFeature => Err(CliError::UnsupportedLifecycleControl),
        ErrorCode::LifecycleBusy if operation == LifecycleOperation::Stop => {
            Err(CliError::LifecycleBusy)
        }
        _ => Err(CliError::LifecycleService {
            reason: "Forge rejected the lifecycle request",
        }),
    }
}

pub(super) fn lifecycle_connect_error(error: &ClientSessionError) -> CliError {
    if matches!(error, ClientSessionError::Handshake(_)) {
        CliError::LifecycleAmbiguous
    } else {
        CliError::LifecycleService {
            reason: "lifecycle connection failed",
        }
    }
}

pub(super) fn lifecycle_request_error(error: &ClientRequestError) -> CliError {
    match error {
        ClientRequestError::UnsupportedFeature => CliError::UnsupportedLifecycleControl,
        ClientRequestError::Exchange(_) => CliError::LifecycleService {
            reason: "lifecycle response exchange failed",
        },
        ClientRequestError::Reply(_) => CliError::LifecycleService {
            reason: "lifecycle response was invalid",
        },
        ClientRequestError::NotARequest { .. }
        | ClientRequestError::VersionMismatch { .. }
        | ClientRequestError::Correlation(_)
        | ClientRequestError::Admission(_) => CliError::LifecycleService {
            reason: "lifecycle request was invalid",
        },
    }
}

pub(super) fn lifecycle_request_requires_quarantine(error: &ClientRequestError) -> bool {
    matches!(
        error,
        ClientRequestError::Correlation(_)
            | ClientRequestError::Exchange(_)
            | ClientRequestError::Reply(_)
    )
}

fn lifecycle_response_requires_quarantine(
    expected_request_id: &RequestId,
    resolved: &artisan_transport::ResolvedRequest,
    result: &Result<LifecycleResult>,
) -> bool {
    if resolved.request_id() != expected_request_id {
        return true;
    }
    matches!(resolved.outcome(), RequestOutcome::Response(_)) && result.is_err()
}

pub(super) fn lifecycle_credential_error(error: &ForgeCredentialError) -> CliError {
    match error {
        ForgeCredentialError::CapabilityBusy
        | ForgeCredentialError::ReconnectCapabilityUnavailable
        | ForgeCredentialError::ReconnectBindingMismatch
        | ForgeCredentialError::ReconnectStaleWriter
        | ForgeCredentialError::ReconnectGenerationOverflow
        | ForgeCredentialError::ReconnectInvalidBinding
        | ForgeCredentialError::ReconnectAttemptComplete
        | ForgeCredentialError::ReconnectRecordExists => CliError::LifecycleCustody {
            reason: "reconnect capability custody is unavailable",
        },
        _ => CliError::LifecycleCredentialState {
            reason: "reconnect capability or client identity is unavailable",
        },
    }
}

/// Readiness of a Forge whose control session its Editor holds: running and
/// where, without the activity only that session may ask for.
fn print_readiness_status(readiness: &process::ForgeReadiness, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "certificate_sha256": readiness.certificate_sha256(),
                "endpoint": readiness.endpoint(),
                "lifecycle": null,
                "pid": readiness.pid(),
                "readiness": "ready",
                "schema": readiness.schema(),
            })
        );
    } else {
        println!(
            "ready (pid {} at {})",
            readiness.pid(),
            readiness.endpoint()
        );
        println!("lifecycle: reported to the Editor that holds its control session");
    }
}

fn print_lifecycle_status(
    readiness: &process::ForgeReadiness,
    lifecycle: &LifecycleStatus,
    json: bool,
) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "certificate_sha256": readiness.certificate_sha256(),
                "endpoint": readiness.endpoint(),
                "lifecycle": {
                    "active_work_count": lifecycle.active_work_count,
                    "state": lifecycle_state_name(lifecycle.state),
                },
                "pid": readiness.pid(),
                "readiness": "ready",
                "schema": readiness.schema(),
            })
        );
    } else {
        println!(
            "ready (pid {} at {})",
            readiness.pid(),
            readiness.endpoint()
        );
        println!(
            "lifecycle: {} ({} active work item(s))",
            lifecycle_state_name(lifecycle.state),
            lifecycle.active_work_count
        );
    }
}

fn print_stop_receipt(receipt: &LifecycleStopReceipt) {
    match receipt.disposition {
        LifecycleStopDisposition::Accepted => println!("stop accepted (draining)"),
        LifecycleStopDisposition::Duplicate | LifecycleStopDisposition::AlreadyStopping => {
            println!("stop already in progress (draining)");
        }
    }
}

pub(super) const fn lifecycle_state_name(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Ready => "ready",
        LifecycleState::Busy => "busy",
        LifecycleState::Draining => "draining",
    }
}

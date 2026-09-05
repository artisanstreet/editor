//! Correlated composer operations over the existing authenticated transport.
use super::*;

pub(super) async fn read_active_run(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    generation: u64,
) -> Result<(), ServiceFailure> {
    let result = if runtime.known_threads.contains(&thread_id) {
        runtime
            .request(
                frames,
                query_request(Query::ReadActiveRun(artisan_domain::ReadActiveRun::new(
                    thread_id.clone(),
                ))),
                ExpectedResponse::ActiveRun(thread_id.clone()),
            )
            .await
            .map_err(ServiceFailure::from)
    } else {
        Err(ServiceFailure::invalid(ServiceFailureStage::Request))
    };
    match result {
        Ok(ResponsePayload::ActiveRun(result)) => publish(
            events,
            NativeTransportEvent::ActiveRun {
                thread_id,
                generation,
                result,
            },
        ),
        Ok(_) => publish(
            events,
            NativeTransportEvent::ActiveRunFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        ),
        Err(failure) => publish(
            events,
            NativeTransportEvent::ActiveRunFailed {
                thread_id,
                generation,
                failure,
            },
        ),
    }
}

pub(super) async fn stop_run(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: artisan_domain::StopRun,
) -> Result<(), ServiceFailure> {
    let result = async {
        known_thread_for_queue(&runtime.known_threads, &command.thread_id)?;
        let frame_id = FrameId::parse(command.request_id.as_str().to_owned())
            .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
        let sent_at = real_unix_millis()
            .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
        let mutation = StableMutation {
            frame_id,
            sent_at,
            command: Command::StopRun(command.clone()),
        };
        // Retrying the exact immutable pair cannot signal a replacement run.
        durable_save_request(
            runtime,
            frames,
            &mutation,
            ExpectedResponse::RunStopped(command.clone()),
        )
        .await
        .map_err(ServiceFailure::from)
    }
    .await;
    match result {
        Ok(ResponsePayload::RunStopped(receipt)) => {
            publish(events, NativeTransportEvent::RunStopped(receipt))
        }
        Ok(_) => publish(
            events,
            NativeTransportEvent::StopRunFailed {
                command,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        ),
        Err(failure) => publish(
            events,
            NativeTransportEvent::StopRunFailed { command, failure },
        ),
    }
}

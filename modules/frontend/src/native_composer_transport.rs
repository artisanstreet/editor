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

pub(super) async fn read_composer_catalog(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    profile_id: artisan_domain::EngineProfileId,
    generation: CatalogLoadGeneration,
) -> Result<(), ServiceFailure> {
    let result = if runtime.known_threads.contains(&thread_id) {
        runtime
            .request(
                frames,
                composer_catalog_request(thread_id.clone(), profile_id.clone()),
                ExpectedResponse::ComposerCatalog {
                    thread_id: thread_id.clone(),
                    profile_id: profile_id.clone(),
                },
            )
            .await
            .map_err(ServiceFailure::from)
    } else {
        Err(ServiceFailure::invalid(ServiceFailureStage::Request))
    };
    match result {
        Ok(ResponsePayload::ComposerCatalog(result)) => publish(
            events,
            NativeTransportEvent::ComposerCatalog {
                thread_id,
                profile_id,
                generation,
                result,
            },
        ),
        Ok(_) => publish(
            events,
            NativeTransportEvent::ComposerCatalogFailed {
                thread_id,
                profile_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        ),
        Err(failure) => publish(
            events,
            NativeTransportEvent::ComposerCatalogFailed {
                thread_id,
                profile_id,
                generation,
                failure,
            },
        ),
    }
}

pub(super) async fn read_model_favorites(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    profile_id: artisan_domain::EngineProfileId,
    generation: CatalogLoadGeneration,
) -> Result<(), ServiceFailure> {
    let result = if runtime.known_threads.contains(&thread_id) {
        runtime
            .request(
                frames,
                model_favorites_request(),
                ExpectedResponse::ModelFavorites,
            )
            .await
            .map_err(ServiceFailure::from)
    } else {
        Err(ServiceFailure::invalid(ServiceFailureStage::Request))
    };
    match result {
        Ok(ResponsePayload::ModelFavorites(result)) => publish(
            events,
            NativeTransportEvent::ModelFavorites {
                thread_id,
                profile_id,
                generation,
                result,
            },
        ),
        Ok(_) => publish(
            events,
            NativeTransportEvent::ModelFavoritesFailed {
                thread_id,
                profile_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        ),
        Err(failure) => publish(
            events,
            NativeTransportEvent::ModelFavoritesFailed {
                thread_id,
                profile_id,
                generation,
                failure,
            },
        ),
    }
}

pub(super) async fn set_model_favorite(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: artisan_domain::SetModelFavorite,
) -> Result<(), ServiceFailure> {
    let thread_id = command.thread_id.clone();
    let profile_id = command.profile_id.clone();
    let request_id = command.request_id.clone();
    let model_id = command.model_id.clone();
    let favorite = command.favorite;
    if !runtime.known_threads.contains(&thread_id) {
        return publish(
            events,
            NativeTransportEvent::ModelFavoriteFailed {
                thread_id,
                profile_id,
                request_id,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    let mutation = match model_favorite_stable_mutation(command) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return publish(
                events,
                NativeTransportEvent::ModelFavoriteFailed {
                    thread_id,
                    profile_id,
                    request_id,
                    failure,
                },
            );
        }
    };
    let payload = match durable_save_request(
        runtime,
        frames,
        &mutation,
        ExpectedResponse::ModelFavoriteSet {
            request_id: request_id.clone(),
            model_id: model_id.clone(),
            favorite,
        },
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return publish(
                events,
                NativeTransportEvent::ModelFavoriteFailed {
                    thread_id,
                    profile_id,
                    request_id,
                    failure: error.into(),
                },
            );
        }
    };
    let ResponsePayload::ModelFavoriteSet(receipt) = payload else {
        return publish(
            events,
            NativeTransportEvent::ModelFavoriteFailed {
                thread_id,
                profile_id,
                request_id,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    if receipt.request_id != request_id
        || receipt.model_id != model_id
        || receipt.favorite != favorite
    {
        return publish(
            events,
            NativeTransportEvent::ModelFavoriteFailed {
                thread_id,
                profile_id,
                request_id,
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ),
            },
        );
    }
    publish(
        events,
        NativeTransportEvent::ModelFavoriteSet {
            thread_id,
            profile_id,
            request_id,
            receipt,
        },
    )
}

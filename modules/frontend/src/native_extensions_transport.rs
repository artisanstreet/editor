//! Transport-thread child for agent extensions and the global prompt: the
//! reads the Marketplace and Settings make, enabling and adopting skills,
//! and saving or settling the global prompt.
//!
//! Every command is answered by exactly one [`ExtensionsEvent`]. After the
//! first read the Forge pushes changes as
//! [`HostStateEvent::Extensions`](super::HostStateEvent::Extensions) and
//! [`HostStateEvent::GlobalPrompt`](super::HostStateEvent::GlobalPrompt).

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    AdoptExtension, ExtensionCatalog, GlobalPrompt, ReadExtensions, ReadGlobalPrompt,
    ResolveGlobalPrompt, SaveGlobalPrompt, SetExtensionEnabled,
};

use super::*;

/// One extension or global prompt command sent from the application thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtensionsCommand {
    /// Read the extension catalog.
    ReadCatalog,
    /// Read the global prompt.
    ReadPrompt,
    /// Turn a managed extension on or off.
    SetEnabled(SetExtensionEnabled),
    /// Adopt a discovered skill.
    Adopt(AdoptExtension),
    /// Save a new version of the global prompt.
    SavePrompt(SaveGlobalPrompt),
    /// Settle setup or a drifted engine.
    ResolvePrompt(ResolveGlobalPrompt),
}

/// One result returned by the service child.
#[derive(Clone, Debug, PartialEq)]
pub enum ExtensionsEvent {
    /// The catalog as read.
    Catalog(Result<Box<ExtensionCatalog>, ServiceFailure>),
    /// The global prompt as read.
    Prompt(Result<Box<GlobalPrompt>, ServiceFailure>),
    /// The catalog after an enable or adopt request, or its failure.
    CatalogChanged {
        /// The request this answers.
        request_id: RequestId,
        /// The catalog after the change, or the failure.
        result: Result<Box<ExtensionCatalog>, ServiceFailure>,
    },
    /// The global prompt after a save or resolution, or its failure.
    PromptChanged {
        /// The request this answers.
        request_id: RequestId,
        /// The prompt after the change, or the failure.
        result: Result<Box<GlobalPrompt>, ServiceFailure>,
    },
}

/// The response shape one request accepts.
#[derive(Clone, Copy)]
pub(super) enum ExtensionsExpectation {
    Catalog,
    Prompt,
}

impl ExtensionsExpectation {
    /// Whether `payload` is the exact answer to this request.
    pub(super) const fn accepts(self, payload: &ResponsePayload) -> bool {
        matches!(
            (self, payload),
            (Self::Catalog, ResponsePayload::Extensions(_))
                | (Self::Prompt, ResponsePayload::GlobalPrompt(_))
        )
    }
}

/// Handles one command on the authenticated service runtime.
pub(super) async fn handle_extensions_command(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: ExtensionsCommand,
) -> Result<(), ServiceFailure> {
    let event = match command {
        ExtensionsCommand::ReadCatalog => ExtensionsEvent::Catalog(
            read(
                runtime,
                frames,
                Query::ReadExtensions(ReadExtensions),
                ExtensionsExpectation::Catalog,
            )
            .await
            .and_then(catalog),
        ),
        ExtensionsCommand::ReadPrompt => ExtensionsEvent::Prompt(
            read(
                runtime,
                frames,
                Query::ReadGlobalPrompt(ReadGlobalPrompt),
                ExtensionsExpectation::Prompt,
            )
            .await
            .and_then(prompt),
        ),
        ExtensionsCommand::SetEnabled(change) => {
            let request_id = change.request_id.clone();
            let result = mutate(
                runtime,
                frames,
                &request_id,
                Command::SetExtensionEnabled(change),
                ExtensionsExpectation::Catalog,
            )
            .await
            .and_then(catalog);
            ExtensionsEvent::CatalogChanged { request_id, result }
        }
        ExtensionsCommand::Adopt(adopt) => {
            let request_id = adopt.request_id.clone();
            let result = mutate(
                runtime,
                frames,
                &request_id,
                Command::AdoptExtension(adopt),
                ExtensionsExpectation::Catalog,
            )
            .await
            .and_then(catalog);
            ExtensionsEvent::CatalogChanged { request_id, result }
        }
        ExtensionsCommand::SavePrompt(save) => {
            let request_id = save.request_id.clone();
            let result = mutate(
                runtime,
                frames,
                &request_id,
                Command::SaveGlobalPrompt(save),
                ExtensionsExpectation::Prompt,
            )
            .await
            .and_then(prompt);
            ExtensionsEvent::PromptChanged { request_id, result }
        }
        ExtensionsCommand::ResolvePrompt(resolve) => {
            let request_id = resolve.request_id.clone();
            let result = mutate(
                runtime,
                frames,
                &request_id,
                Command::ResolveGlobalPrompt(resolve),
                ExtensionsExpectation::Prompt,
            )
            .await
            .and_then(prompt);
            ExtensionsEvent::PromptChanged { request_id, result }
        }
    };
    publish(events, NativeTransportEvent::Extensions(event))
}

fn catalog(payload: ResponsePayload) -> Result<Box<ExtensionCatalog>, ServiceFailure> {
    match payload {
        ResponsePayload::Extensions(catalog) => Ok(Box::new(catalog)),
        _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
    }
}

fn prompt(payload: ResponsePayload) -> Result<Box<GlobalPrompt>, ServiceFailure> {
    match payload {
        ResponsePayload::GlobalPrompt(prompt) => Ok(Box::new(prompt)),
        _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
    }
}

async fn read(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    query: Query,
    expected: ExtensionsExpectation,
) -> Result<ResponsePayload, ServiceFailure> {
    Box::pin(runtime.request(
        frames,
        query_request(query),
        ExpectedResponse::Extensions(expected),
    ))
    .await
    .map_err(ServiceFailure::from)
}

/// Sends one mutation with its stable frame identity.
async fn mutate(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    request_id: &RequestId,
    command: Command,
    expected: ExtensionsExpectation,
) -> Result<ResponsePayload, ServiceFailure> {
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let mutation = StableMutation {
        frame_id,
        sent_at,
        command,
    };
    Box::pin(durable_save_request(
        runtime,
        frames,
        &mutation,
        ExpectedResponse::Extensions(expected),
    ))
    .await
    .map_err(ServiceFailure::from)
}

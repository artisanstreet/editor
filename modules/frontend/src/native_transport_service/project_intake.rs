//! Project-intake flows for the native transport service: directory picking,
//! attach, project and thread refresh, and the one retained retry plan.
//!
//! The parent `native_transport_service` remains the owner of the command and
//! event vocabulary, intake state, and the bounded bridge publication helper.
//! Root mounts this file as a child module so the intake state machine stays
//! in one reviewable home.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use super::*;

/// One private retry plan for a project intake. Stable mutations retain their
/// complete command identity; reads and the picker are intentionally retried
/// with fresh frames.
pub(super) enum IntakeRetry {
    Validate(String),
    Pick,
    Attach(StableMutation),
    RefreshProjects {
        attached: ProjectSummary,
    },
    Create(StableMutation),
    RefreshThreads {
        project_id: ProjectId,
        created: ThreadSummary,
    },
}

pub(super) struct IntakeState {
    pub(super) selected_directory: Option<DirectoryId>,
    pub(super) projects: Option<ProjectListing>,
    pub(super) retry: Option<IntakeRetry>,
}

impl IntakeState {
    pub(super) const fn new() -> Self {
        Self {
            selected_directory: None,
            projects: None,
            retry: None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.selected_directory = None;
        self.projects = None;
        self.retry = None;
    }
}

pub(super) async fn begin_project_intake(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    runtime.intake.reset();
    pick_directory(runtime, frames, events, None).await
}

pub(super) async fn retry_project_intake(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    let Some(retry) = runtime.intake.retry.take() else {
        return Ok(());
    };
    match retry {
        IntakeRetry::Validate(path) => begin_project_intake_at(runtime, frames, events, path).await,
        IntakeRetry::Pick => {
            runtime.intake.selected_directory = None;
            runtime.intake.projects = None;
            pick_directory(runtime, frames, events, None).await
        }
        IntakeRetry::Attach(mutation) => {
            attach_project_with_mutation(runtime, frames, events, mutation, true).await
        }
        IntakeRetry::RefreshProjects { attached } => {
            refresh_projects(runtime, frames, events, attached).await
        }
        IntakeRetry::Create(mutation) => {
            let Some(projects) = runtime.intake.projects.clone() else {
                return report_intake_failure(
                    runtime,
                    events,
                    NativeProjectIntakeOperation::CreateThread,
                    RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
                    None,
                    false,
                );
            };
            let Some((project_id, title)) = create_command_values(&mutation) else {
                return report_intake_failure(
                    runtime,
                    events,
                    NativeProjectIntakeOperation::CreateThread,
                    RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
                    None,
                    false,
                );
            };
            create_thread_with_mutation(
                runtime, frames, events, projects, project_id, title, mutation, true,
            )
            .await
        }
        IntakeRetry::RefreshThreads {
            project_id,
            created,
        } => refresh_threads(runtime, frames, events, project_id, created).await,
    }
}

pub(super) async fn begin_project_intake_at(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    path: String,
) -> Result<(), ServiceFailure> {
    runtime.intake.reset();
    pick_directory(runtime, frames, events, Some(path)).await
}

async fn pick_directory(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    selected_path: Option<String>,
) -> Result<(), ServiceFailure> {
    runtime.intake.selected_directory = None;
    runtime.intake.projects = None;
    publish(
        events,
        NativeTransportEvent::ProjectIntakeProgress(NativeProjectIntakeStage::PickingDirectory),
    )?;
    let request = match selected_path.as_ref() {
        Some(path) => ClientRequest::ValidateDirectory(
            artisan_domain::RootPath::parse(path.clone())
                .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?,
        ),
        None => ClientRequest::PickDirectory,
    };
    let payload = match runtime
        .request(frames, request, ExpectedResponse::Directory)
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::PickDirectory,
                error,
                Some(selected_path.map_or(IntakeRetry::Pick, IntakeRetry::Validate)),
                true,
            );
        }
    };
    match payload {
        ResponsePayload::DirectoryPicked(artisan_protocol::DirectoryPickOutcome::Selected(
            directory_id,
        )) => {
            runtime.intake.selected_directory = Some(directory_id);
            attach_project(runtime, frames, events).await
        }
        ResponsePayload::DirectoryPicked(artisan_protocol::DirectoryPickOutcome::Cancelled) => {
            runtime.intake.reset();
            publish(events, NativeTransportEvent::ProjectIntakeCancelled)
        }
        _ => report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::PickDirectory,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        ),
    }
}

async fn attach_project(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    let Some(directory_id) = runtime.intake.selected_directory.clone() else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::AttachProject,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    let mutation = match attach_mutation(frames, directory_id) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::AttachProject,
                RequestFailure::terminal(failure),
                None,
                false,
            );
        }
    };
    attach_project_with_mutation(runtime, frames, events, mutation, false).await
}

async fn attach_project_with_mutation(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    mutation: StableMutation,
    force_reconnect: bool,
) -> Result<(), ServiceFailure> {
    publish(
        events,
        NativeTransportEvent::ProjectIntakeProgress(NativeProjectIntakeStage::AttachingProject),
    )?;
    let payload = match runtime
        .request_stable(
            frames,
            &mutation,
            ExpectedResponse::AttachedProject,
            force_reconnect,
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            let allow_retry = attach_retry_allowed(error);
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::AttachProject,
                error,
                Some(IntakeRetry::Attach(mutation)),
                allow_retry,
            );
        }
    };
    let ResponsePayload::AttachedProject {
        project,
        disposition: _,
    } = payload
    else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::AttachProject,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    refresh_projects(runtime, frames, events, project).await
}

async fn refresh_projects(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    attached: ProjectSummary,
) -> Result<(), ServiceFailure> {
    publish(
        events,
        NativeTransportEvent::ProjectIntakeProgress(NativeProjectIntakeStage::RefreshingProjects),
    )?;
    let payload = match runtime
        .request(frames, project_request(), ExpectedResponse::Projects)
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::RefreshProjects,
                error,
                Some(IntakeRetry::RefreshProjects { attached }),
                true,
            );
        }
    };
    let ResponsePayload::ProjectListing(projects) = payload else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::RefreshProjects,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    if !contains_exact_project(&projects, &attached) {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::RefreshProjects,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    }
    runtime.intake.projects = Some(projects.clone());
    let Ok(title) = ThreadTitle::parse("New thread") else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::CreateThread,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    create_thread(
        runtime,
        frames,
        events,
        projects,
        attached.project_id.clone(),
        title,
    )
    .await
}

pub(super) async fn create_task_in_project(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    project_id: ProjectId,
) -> Result<(), ServiceFailure> {
    runtime.intake.reset();
    let Some(projects) = read_projects_containing(runtime, frames, events, &project_id).await?
    else {
        return Ok(());
    };
    let title = ThreadTitle::parse("New task").expect("static task title is valid");
    create_thread(runtime, frames, events, projects, project_id, title).await
}

/// Reads the attached projects for a task in `project_id`. A failure is
/// reported as a failed thread creation and answers `None`.
async fn read_projects_containing(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    project_id: &ProjectId,
) -> Result<Option<ProjectListing>, ServiceFailure> {
    let error = match runtime
        .request(frames, project_request(), ExpectedResponse::Projects)
        .await
    {
        Ok(ResponsePayload::ProjectListing(projects))
            if projects
                .projects()
                .iter()
                .any(|project| &project.project_id == project_id) =>
        {
            runtime.intake.projects = Some(projects.clone());
            return Ok(Some(projects));
        }
        Ok(_) => RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
        Err(error) => error,
    };
    report_intake_failure(
        runtime,
        events,
        NativeProjectIntakeOperation::CreateThread,
        error,
        None,
        false,
    )
    .map(|()| None)
}

/// Asks the Forge to move one failed message into a new thread of its
/// project, then opens that thread exactly like a created task. The Forge
/// stores the prompt as the new thread's draft; nothing is sent.
pub(super) async fn recover_failed_message(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    project_id: ProjectId,
    command: artisan_domain::RecoverFailedMessage,
) -> Result<(), ServiceFailure> {
    runtime.intake.reset();
    if read_projects_containing(runtime, frames, events, &project_id)
        .await?
        .is_none()
    {
        return Ok(());
    }
    publish(
        events,
        NativeTransportEvent::ProjectIntakeProgress(NativeProjectIntakeStage::CreatingThread),
    )?;
    let invalid =
        || RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request));
    let outcome = match super::composer_state_operations::stable_mutation(
        &command.request_id,
        Command::RecoverFailedMessage(command.clone()),
    ) {
        Ok(mutation) => {
            runtime
                .request_stable(
                    frames,
                    &mutation,
                    ExpectedResponse::FailedMessageRecovered {
                        request_id: command.request_id.clone(),
                    },
                    false,
                )
                .await
        }
        Err(failure) => Err(RequestFailure::terminal(failure)),
    };
    let thread_id = match outcome {
        Ok(ResponsePayload::FailedMessageRecovered(recovered))
            if recovered.target == command.target =>
        {
            recovered.new_thread_id
        }
        Ok(_) => None,
        Err(error) => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::CreateThread,
                error,
                None,
                false,
            );
        }
    };
    let Some(thread_id) = thread_id else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::CreateThread,
            invalid(),
            None,
            false,
        );
    };
    publish(
        events,
        NativeTransportEvent::ProjectIntakeProgress(NativeProjectIntakeStage::RefreshingThreads),
    )?;
    let threads = match runtime
        .request(
            frames,
            threads_request(project_id.clone()),
            ExpectedResponse::Threads(project_id.clone()),
        )
        .await
    {
        Ok(ResponsePayload::ThreadListing(threads))
            if threads
                .threads()
                .iter()
                .any(|thread| thread.thread_id == thread_id && thread.project_id == project_id) =>
        {
            threads
        }
        other => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::RefreshThreads,
                other.err().unwrap_or_else(invalid),
                None,
                false,
            );
        }
    };
    finish_intake(runtime, events, project_id, threads, thread_id)
}

async fn create_thread(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    projects: ProjectListing,
    project_id: ProjectId,
    title: ThreadTitle,
) -> Result<(), ServiceFailure> {
    let mutation = match create_mutation(frames, project_id.clone(), title.clone()) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::CreateThread,
                RequestFailure::terminal(failure),
                None,
                false,
            );
        }
    };
    create_thread_with_mutation(
        runtime, frames, events, projects, project_id, title, mutation, false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn create_thread_with_mutation(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    projects: ProjectListing,
    project_id: ProjectId,
    title: ThreadTitle,
    mutation: StableMutation,
    force_reconnect: bool,
) -> Result<(), ServiceFailure> {
    publish(
        events,
        NativeTransportEvent::ProjectIntakeProgress(NativeProjectIntakeStage::CreatingThread),
    )?;
    let payload = match runtime
        .request_stable(
            frames,
            &mutation,
            ExpectedResponse::CreatedThread,
            force_reconnect,
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::CreateThread,
                error,
                Some(IntakeRetry::Create(mutation)),
                true,
            );
        }
    };
    let ResponsePayload::CreatedThread {
        thread,
        disposition: _,
    } = payload
    else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::CreateThread,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    if thread.project_id != project_id || thread.title != title {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::CreateThread,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    }
    runtime.intake.projects = Some(projects);
    refresh_threads(runtime, frames, events, project_id, thread).await
}

pub(super) fn create_command_values(mutation: &StableMutation) -> Option<(ProjectId, ThreadTitle)> {
    match &mutation.command {
        Command::CreateThread(command) => Some((command.project_id.clone(), command.title.clone())),
        _ => None,
    }
}

async fn refresh_threads(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    project_id: ProjectId,
    created: ThreadSummary,
) -> Result<(), ServiceFailure> {
    let payload = {
        publish(
            events,
            NativeTransportEvent::ProjectIntakeProgress(
                NativeProjectIntakeStage::RefreshingThreads,
            ),
        )?;
        match runtime
            .request(
                frames,
                threads_request(project_id.clone()),
                ExpectedResponse::Threads(project_id.clone()),
            )
            .await
        {
            Ok(payload) => payload,
            Err(error) => {
                return report_intake_failure(
                    runtime,
                    events,
                    NativeProjectIntakeOperation::RefreshThreads,
                    error,
                    Some(IntakeRetry::RefreshThreads {
                        project_id,
                        created,
                    }),
                    true,
                );
            }
        }
    };
    let ResponsePayload::ThreadListing(threads) = payload else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::RefreshThreads,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    if !contains_exact_thread(&threads, &created) {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::RefreshThreads,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    }
    finish_intake(runtime, events, project_id, threads, created.thread_id)
}

/// Completes an intake with the authoritative listings and opens `thread_id`.
fn finish_intake(
    runtime: &mut ServiceRuntime,
    events: &SyncSender<NativeTransportEvent>,
    project_id: ProjectId,
    threads: ThreadListing,
    thread_id: ThreadId,
) -> Result<(), ServiceFailure> {
    let Some(projects) = runtime.intake.projects.clone() else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::RefreshThreads,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    runtime.known_threads.clear();
    runtime.known_threads.extend(
        threads
            .threads()
            .iter()
            .map(|thread| thread.thread_id.clone()),
    );
    runtime.intake.reset();
    publish(
        events,
        NativeTransportEvent::ProjectIntakeReady {
            projects,
            project_id,
            threads,
            thread_id,
        },
    )
}

fn report_intake_failure(
    runtime: &mut ServiceRuntime,
    events: &SyncSender<NativeTransportEvent>,
    operation: NativeProjectIntakeOperation,
    error: RequestFailure,
    retry: Option<IntakeRetry>,
    allow_retry: bool,
) -> Result<(), ServiceFailure> {
    let retryable = retry.is_some() && report_retry_allowed(error, allow_retry);
    runtime.intake.retry = if retryable { retry } else { None };
    publish(
        events,
        NativeTransportEvent::ProjectIntakeFailed {
            operation,
            failure: error.failure,
            retryable,
        },
    )
}

fn report_retry_allowed(error: RequestFailure, allow_retry: bool) -> bool {
    allow_retry && error.retryable()
}

pub(super) fn attach_retry_allowed(error: RequestFailure) -> bool {
    error.code() != Some(ErrorCode::DirectoryUnknown) && error.retryable()
}

pub(super) fn contains_exact_project(projects: &ProjectListing, expected: &ProjectSummary) -> bool {
    projects.projects().contains(expected)
}

/// Verify durable creation fields; live catalog metadata can change after creation.
pub(super) fn contains_exact_thread(threads: &ThreadListing, expected: &ThreadSummary) -> bool {
    threads.threads().iter().any(|thread| {
        thread.thread_id == expected.thread_id
            && thread.project_id == expected.project_id
            && thread.title == expected.title
            && thread.created_at == expected.created_at
            && thread.updated_at == expected.updated_at
    })
}

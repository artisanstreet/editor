//! Payload-free event names for the flight recorder. Exhaustive matches
//! make new protocol events visible without serializing their contents.

use super::*;

pub(super) fn monitor(runtime: &tokio::runtime::Runtime) {
    if !artisan_tracing::is_recording() {
        return;
    }
    let heartbeat = artisan_tracing::Heartbeat::new("editor.transport");
    runtime.spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_millis(100));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            heartbeat.tick();
        }
    });
}

impl NativeTransportCommand {
    pub(crate) fn trace_thread(&self) -> Option<&str> {
        match self {
            Self::Subscribe { thread_id, .. }
            | Self::Unsubscribe { thread_id }
            | Self::ReadConversationHistory { thread_id, .. }
            | Self::LoadThreadEngineSettings { thread_id, .. }
            | Self::ReadComposerCatalog { thread_id, .. }
            | Self::ReadModelFavorites { thread_id, .. }
            | Self::ReadActiveRun { thread_id, .. } => Some(thread_id.as_str()),
            Self::RequestSnapshot(thread_id) => Some(thread_id.as_str()),
            _ => None,
        }
    }

    pub(crate) fn trace_name(&self) -> &'static str {
        match self {
            Self::ComposerState(..) => "ComposerState",
            Self::ComposerDraft(..) => "ComposerDraft",
            Self::ForgeDecision(..) => "ForgeDecision",
            Self::Preferences(..) => "Preferences",
            Self::EngineInstalls(..) => "EngineInstalls",
            Self::ReadActiveRun { .. } => "ReadActiveRun",
            Self::StopRun(..) => "StopRun",
            Self::RespondApproval(..) => "RespondApproval",
            Self::RespondQuestion(..) => "RespondQuestion",
            Self::AnswerQuestions(..) => "AnswerQuestions",
            Self::BeginProjectIntake => "BeginProjectIntake",
            Self::BeginProjectIntakeAt(..) => "BeginProjectIntakeAt",
            Self::RetryProjectIntake => "RetryProjectIntake",
            Self::SelectProject(..) => "SelectProject",
            Self::RefreshThreads { .. } => "RefreshThreads",
            Self::ReadRecentThreads => "ReadRecentThreads",
            Self::ReadProjects => "ReadProjects",
            Self::CreateTask(..) => "CreateTask",
            Self::RecoverFailedMessage { .. } => "RecoverFailedMessage",
            Self::RequestSnapshot(..) => "RequestSnapshot",
            Self::ReadConversationHistory { .. } => "ReadConversationHistory",
            Self::ReadMessageImage(..) => "ReadMessageImage",
            Self::LoadThreadEngineSettings { .. } => "LoadThreadEngineSettings",
            Self::ReadComposerCatalog { .. } => "ReadComposerCatalog",
            Self::ReadModelFavorites { .. } => "ReadModelFavorites",
            Self::ListRegisteredProfiles => "ListRegisteredProfiles",
            Self::ReadAccountUsage { .. } => "ReadAccountUsage",
            Self::SetThreadEngineConfig(..) => "SetThreadEngineConfig",
            Self::SetModelFavorite(..) => "SetModelFavorite",
            Self::SubmitComposerDraft(..) => "SubmitComposerDraft",
            Self::ResolveRichLink { .. } => "ResolveRichLink",
            Self::QueryProjectRepository { .. } => "QueryProjectRepository",
            Self::Subscribe { .. } => "Subscribe",
            Self::Unsubscribe { .. } => "Unsubscribe",
            Self::AcknowledgePatch { .. } => "AcknowledgePatch",
            Self::Shutdown => "Shutdown",
        }
    }
}

impl NativeTransportEvent {
    pub(crate) fn trace_name(&self) -> &'static str {
        match self {
            Self::ComposerState(..) => "ComposerState",
            Self::ComposerDraft(..) => "ComposerDraft",
            Self::ForgeDecision(..) => "ForgeDecision",
            Self::Preferences(..) => "Preferences",
            Self::EngineInstalls(..) => "EngineInstalls",
            Self::ActiveRun { .. } => "ActiveRun",
            Self::ActiveRunFailed { .. } => "ActiveRunFailed",
            Self::RunStopped(..) => "RunStopped",
            Self::StopRunFailed { .. } => "StopRunFailed",
            Self::ApprovalAnswered { .. } => "ApprovalAnswered",
            Self::ApprovalFailed { .. } => "ApprovalFailed",
            Self::QuestionAnswered { .. } => "QuestionAnswered",
            Self::QuestionFailed { .. } => "QuestionFailed",
            Self::QuestionsAnswered { .. } => "QuestionsAnswered",
            Self::QuestionsAnswerFailed { .. } => "QuestionsAnswerFailed",
            Self::MessageImageLoaded { .. } => "MessageImageLoaded",
            Self::MessageImageFailed { .. } => "MessageImageFailed",
            Self::Starting => "Starting",
            Self::Projects(..) => "Projects",
            Self::Threads { .. } => "Threads",
            Self::ThreadsRefreshed { .. } => "ThreadsRefreshed",
            Self::RecentThreads(..) => "RecentThreads",
            Self::ProjectCatalog(..) => "ProjectCatalog",
            Self::Snapshot(..) => "Snapshot",
            Self::EmptyProjects => "EmptyProjects",
            Self::EmptyThreads { .. } => "EmptyThreads",
            Self::ProjectIntakeProgress(..) => "ProjectIntakeProgress",
            Self::ProjectIntakeCancelled => "ProjectIntakeCancelled",
            Self::ProjectIntakeReady { .. } => "ProjectIntakeReady",
            Self::ProjectIntakeFailed { .. } => "ProjectIntakeFailed",
            Self::Failed(..) => "Failed",
            Self::ThreadEngineSettings { .. } => "ThreadEngineSettings",
            Self::RegisteredProfiles(..) => "RegisteredProfiles",
            Self::RegisteredProfilesFailed(..) => "RegisteredProfilesFailed",
            Self::AccountUsage { .. } => "AccountUsage",
            Self::AccountUsageFailed { .. } => "AccountUsageFailed",
            Self::ComposerCatalog { .. } => "ComposerCatalog",
            Self::ComposerCatalogFailed { .. } => "ComposerCatalogFailed",
            Self::ModelFavorites { .. } => "ModelFavorites",
            Self::ModelFavoritesFailed { .. } => "ModelFavoritesFailed",
            Self::ModelFavoriteSet { .. } => "ModelFavoriteSet",
            Self::RichLinkResolved { .. } => "RichLinkResolved",
            Self::RichLinkFailed { .. } => "RichLinkFailed",
            Self::ProjectRepository { .. } => "ProjectRepository",
            Self::ProjectRepositoryFailed { .. } => "ProjectRepositoryFailed",
            Self::ModelFavoriteFailed { .. } => "ModelFavoriteFailed",
            Self::ThreadEngineConfigSet(..) => "ThreadEngineConfigSet",
            Self::ThreadEngineConfigConflict { .. } => "ThreadEngineConfigConflict",
            Self::ThreadEngineConfigFailed { .. } => "ThreadEngineConfigFailed",
            Self::MessageQueued(..) => "MessageQueued",
            Self::MessageFailed { .. } => "MessageFailed",
            Self::MessageStale { .. } => "MessageStale",
            Self::ThreadEngineSettingsFailed { .. } => "ThreadEngineSettingsFailed",
            Self::ConversationSubscriptionStarted { .. } => "ConversationSubscriptionStarted",
            Self::ConversationSubscriptionStopped { .. } => "ConversationSubscriptionStopped",
            Self::ConversationHistory { .. } => "ConversationHistory",
            Self::ConversationHistoryFailed { .. } => "ConversationHistoryFailed",
            Self::PatchBatch(..) => "PatchBatch",
            Self::EngineObservation(..) => "EngineObservation",
            Self::MessageOutbox(..) => "MessageOutbox",
            Self::ObservationHistoryCurrent(..) => "ObservationHistoryCurrent",
            Self::HostState(..) => "HostState",
            Self::DeliveryLost(..) => "DeliveryLost",
            Self::Reconnected => "Reconnected",
            Self::HostHome(..) => "HostHome",
            Self::Stopped(..) => "Stopped",
        }
    }
}

impl ExpectedResponse {
    pub(super) fn trace_name(&self) -> &'static str {
        match self {
            Self::MessageWithdrawn { .. } => "MessageWithdrawn",
            Self::FailedMessageRetried { .. } => "FailedMessageRetried",
            Self::FailedMessageRecovered { .. } => "FailedMessageRecovered",
            Self::RunUsage { .. } => "RunUsage",
            Self::ActiveRun(..) => "ActiveRun",
            Self::RunStopped(..) => "RunStopped",
            Self::Directory => "Directory",
            Self::Projects => "Projects",
            Self::RecentThreads => "RecentThreads",
            Self::AttachedProject => "AttachedProject",
            Self::CreatedThread => "CreatedThread",
            Self::Threads(..) => "Threads",
            Self::Snapshot(..) => "Snapshot",
            Self::ConversationHistory(..) => "ConversationHistory",
            Self::MessageImage(..) => "MessageImage",
            Self::ThreadEngineSettings(..) => "ThreadEngineSettings",
            Self::RegisteredProfiles => "RegisteredProfiles",
            Self::AccountUsage { .. } => "AccountUsage",
            Self::ComposerCatalog { .. } => "ComposerCatalog",
            Self::RichLink { .. } => "RichLink",
            Self::ProjectRepository { .. } => "ProjectRepository",
            Self::ModelFavorites => "ModelFavorites",
            Self::ModelFavoriteSet { .. } => "ModelFavoriteSet",
            Self::ThreadEngineConfigSet { .. } => "ThreadEngineConfigSet",
            Self::DraftSubmitted { .. } => "DraftSubmitted",
            Self::ApprovalAnswered { .. } => "ApprovalAnswered",
            Self::QuestionAnswered { .. } => "QuestionAnswered",
            Self::QuestionsAnswered { .. } => "QuestionsAnswered",
            Self::ConversationSubscriptionStarted { .. } => "ConversationSubscriptionStarted",
            Self::ConversationSubscriptionStopped { .. } => "ConversationSubscriptionStopped",
            Self::ComposerDraft(..) => "ComposerDraft",
            Self::ForgeDecision(..) => "ForgeDecision",
            Self::EngineInstalls(..) => "EngineInstalls",
            Self::Preferences(..) => "Preferences",
        }
    }
}

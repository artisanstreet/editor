use crate::server::Failure;
use forge_runs::{ErrorClass, error};
use forge_store::coordination::CoordinationFailure;

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error(transparent)]
    AgentQuery(#[from] forge_runs::AgentQueryError),
    #[error(transparent)]
    Approval(#[from] forge_runs::ApprovalError),
    #[error(transparent)]
    CallEditor(#[from] forge_runs::CallEditorError),
    #[error(transparent)]
    CancelAgent(#[from] forge_runs::CancelAgentError),
    #[error(transparent)]
    Cancel(#[from] forge_runs::CancelError),
    #[error(transparent)]
    DeliverMessage(#[from] forge_runs::DeliverMessageError),
    #[error(transparent)]
    GlobalPrompt(#[from] forge_runs::GlobalPromptError),
    #[error(transparent)]
    InstallSkill(#[from] forge_runs::InstallSkillError),
    #[error(transparent)]
    McpAuth(#[from] forge_runs::McpAuthError),
    #[error(transparent)]
    MessageAgent(#[from] forge_runs::MessageAgentError),
    #[error(transparent)]
    SaveGlobalPrompt(#[from] forge_runs::SaveGlobalPromptError),
    #[error(transparent)]
    SendAsAgent(#[from] forge_runs::SendAsAgentError),
    #[error(transparent)]
    SetSkillEnabled(#[from] forge_runs::SetSkillEnabledError),
    #[error(transparent)]
    SkillCatalog(#[from] forge_runs::SkillCatalogError),
    #[error(transparent)]
    SpawnAgent(#[from] forge_runs::SpawnAgentError),
    #[error(transparent)]
    StartThread(#[from] forge_runs::StartThreadError),
    #[error(transparent)]
    AwaitingItems(#[from] forge_store::AwaitingItemsError),
    #[error(transparent)]
    ReadItem(#[from] forge_store::ReadItemError),
    #[error(transparent)]
    ReadProject(#[from] forge_store::ReadProjectError),
    #[error(transparent)]
    ReadSetting(#[from] forge_store::ReadSettingError),
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
    #[error(transparent)]
    RemoveProject(#[from] forge_store::RemoveProjectError),
    #[error(transparent)]
    State(#[from] forge_store::StateError),
    #[error(transparent)]
    TranscriptPage(#[from] forge_store::TranscriptPageError),
    #[error(transparent)]
    CreateProject(#[from] forge_store::CreateProjectError),
    #[error(transparent)]
    MarkUnread(#[from] forge_store::MarkUnreadError),
    #[error(transparent)]
    MutateMcp(#[from] forge_store::MutateMcpError),
    #[error(transparent)]
    RecallGrep(#[from] forge_store::RecallGrepError),
    #[error(transparent)]
    RecallSearch(#[from] forge_store::RecallSearchError),
    #[error(transparent)]
    RecallTouching(#[from] forge_store::RecallTouchingError),
    #[error(transparent)]
    SetDefaultModel(#[from] forge_store::SetDefaultModelError),
    #[error(transparent)]
    SetSetting(#[from] forge_store::SetSettingError),
    #[error(transparent)]
    SettleThread(#[from] forge_store::SettleThreadError),
    #[error(transparent)]
    UpdateThread(#[from] forge_store::UpdateThreadError),
    #[error(transparent)]
    CoordinationGet(#[from] forge_store::coordination::CoordinationGetError),
    #[error(transparent)]
    CoordinationList(#[from] forge_store::coordination::CoordinationListError),
    #[error(transparent)]
    CoordinationPut(#[from] forge_store::coordination::CoordinationPutError),
    #[error(transparent)]
    CoordinationUpdate(#[from] forge_store::coordination::CoordinationUpdateError),
    #[error(transparent)]
    CoordinationChanges(#[from] forge_store::coordination::CoordinationChangesError),
    #[error(transparent)]
    DiscoverWorkers(#[from] forge_store::discovery::DiscoverWorkersError),
    #[error(transparent)]
    FileActivity(#[from] forge_store::discovery::FileActivityError),
}

impl ForgeError {
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::AgentQuery(error) => error.class(),
            Self::Approval(error) => error.class(),
            Self::CallEditor(error) => error.class(),
            Self::CancelAgent(error) => error.class(),
            Self::Cancel(error) => error.class(),
            Self::DeliverMessage(error) => error.class(),
            Self::GlobalPrompt(error) => error.class(),
            Self::InstallSkill(error) => error.class(),
            Self::McpAuth(error) => error.class(),
            Self::MessageAgent(error) => error.class(),
            Self::SaveGlobalPrompt(error) => error.class(),
            Self::SendAsAgent(error) => error.class(),
            Self::SetSkillEnabled(error) => error.class(),
            Self::SkillCatalog(error) => error.class(),
            Self::SpawnAgent(error) => error.class(),
            Self::StartThread(error) => error.class(),
            Self::CreateProject(error) => error::create_project(&error.cause),
            Self::MarkUnread(error) => error::write(&error.cause),
            Self::MutateMcp(error) => error::setting(&error.cause),
            Self::RecallGrep(error) => error::grep(&error.cause),
            Self::RecallSearch(error) => error::content(&error.cause),
            Self::RecallTouching(error) => error::content(&error.cause),
            Self::SetDefaultModel(error) => error::setting(&error.cause),
            Self::SetSetting(error) => error::setting(&error.cause),
            Self::SettleThread(error) => error::write(&error.cause),
            Self::UpdateThread(error) => error::update_thread(&error.cause),
            Self::CoordinationGet(error) => error::coordination(&error.cause),
            Self::CoordinationList(error) => error::coordination(&error.cause),
            Self::CoordinationPut(error) => error::coordination(&error.cause),
            Self::CoordinationUpdate(error) => error::coordination(&error.cause),
            Self::ReadThread(_)
            | Self::ReadItem(_)
            | Self::TranscriptPage(_)
            | Self::AwaitingItems(_)
            | Self::ReadSetting(_)
            | Self::State(_)
            | Self::ReadProject(_)
            | Self::RemoveProject(_)
            | Self::CoordinationChanges(_)
            | Self::DiscoverWorkers(_)
            | Self::FileActivity(_) => ErrorClass::Internal,
        }
    }

    pub fn failure(&self) -> Failure<'_> {
        match self {
            Self::CoordinationGet(error) => coordination(&error.cause, self),
            Self::CoordinationList(error) => coordination(&error.cause, self),
            Self::CoordinationPut(error) => coordination(&error.cause, self),
            Self::CoordinationUpdate(error) => coordination(&error.cause, self),
            Self::DeliverMessage(forge_runs::DeliverMessageError::Get(error)) => {
                coordination(&error.cause, self)
            }
            Self::DeliverMessage(forge_runs::DeliverMessageError::Update(error)) => {
                coordination(&error.cause, self)
            }
            Self::DeliverMessage(
                forge_runs::DeliverMessageError::NotFound { .. }
                | forge_runs::DeliverMessageError::NotSender { .. }
                | forge_runs::DeliverMessageError::NotAMessage { .. }
                | forge_runs::DeliverMessageError::Send { .. }
                | forge_runs::DeliverMessageError::Raced { .. },
            )
            | Self::AgentQuery(_)
            | Self::Approval(_)
            | Self::CallEditor(_)
            | Self::CancelAgent(_)
            | Self::Cancel(_)
            | Self::GlobalPrompt(_)
            | Self::InstallSkill(_)
            | Self::McpAuth(_)
            | Self::MessageAgent(_)
            | Self::SaveGlobalPrompt(_)
            | Self::SendAsAgent(_)
            | Self::SetSkillEnabled(_)
            | Self::SkillCatalog(_)
            | Self::SpawnAgent(_)
            | Self::StartThread(_)
            | Self::AwaitingItems(_)
            | Self::ReadItem(_)
            | Self::ReadProject(_)
            | Self::ReadSetting(_)
            | Self::ReadThread(_)
            | Self::RemoveProject(_)
            | Self::State(_)
            | Self::TranscriptPage(_)
            | Self::CreateProject(_)
            | Self::MarkUnread(_)
            | Self::MutateMcp(_)
            | Self::RecallGrep(_)
            | Self::RecallSearch(_)
            | Self::RecallTouching(_)
            | Self::SetDefaultModel(_)
            | Self::SetSetting(_)
            | Self::SettleThread(_)
            | Self::UpdateThread(_)
            | Self::CoordinationChanges(_)
            | Self::DiscoverWorkers(_)
            | Self::FileActivity(_) => Failure::classified(self.class(), self),
        }
    }
}

pub fn coordination<'a>(
    failure: &'a CoordinationFailure,
    whole: &dyn std::error::Error,
) -> Failure<'a> {
    if let CoordinationFailure::Stale(conflict) = failure {
        return Failure::Stale(conflict);
    }
    Failure::classified(error::coordination(failure), whole)
}

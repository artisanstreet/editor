use forge_store::{
    AnswerFailure, ContentError, CreateProjectFailure, CreateThreadError, GrepFailure,
    InputFailure, SendRejected, SettingFailure, SpawnFailure, UpdateThreadFailure, WriteFailure,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    NotFound,
    Conflict,
    BadRequest,
    Internal,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Refused(pub String);

#[must_use]
const fn send_rejected(rejected: &SendRejected) -> ErrorClass {
    match rejected {
        SendRejected::ThreadNotFound(_) => ErrorClass::NotFound,
        SendRejected::IdConflict(_)
        | SendRejected::TextTooLarge { .. }
        | SendRejected::TooManyImages { .. }
        | SendRejected::ImageTooLarge { .. }
        | SendRejected::ImagesTooLarge { .. }
        | SendRejected::MissingBlob(_)
        | SendRejected::InvalidConfig => ErrorClass::BadRequest,
    }
}

#[must_use]
pub const fn write(failure: &WriteFailure) -> ErrorClass {
    match failure {
        WriteFailure::ThreadNotFound { .. } => ErrorClass::NotFound,
        WriteFailure::SettingsFull { .. } => ErrorClass::Conflict,
        WriteFailure::Content(content) => self::content(content),
        WriteFailure::ItemMoved { .. }
        | WriteFailure::NotAnInput { .. }
        | WriteFailure::RunMoved(_)
        | WriteFailure::Storage(_) => ErrorClass::Internal,
    }
}

#[must_use]
pub const fn content(error: &ContentError) -> ErrorClass {
    match error {
        ContentError::Refused(_) => ErrorClass::Conflict,
        ContentError::UnknownReference { .. } | ContentError::Missing { .. } => {
            ErrorClass::NotFound
        }
        ContentError::TextTooLarge { .. }
        | ContentError::FieldTooLong { .. }
        | ContentError::TooManyEntries { .. }
        | ContentError::MetadataTooLarge { .. }
        | ContentError::DataTooLarge { .. }
        | ContentError::InvalidIdentity { .. }
        | ContentError::RangeLimit { .. }
        | ContentError::OffsetPastEnd { .. }
        | ContentError::RecordTooLarge { .. }
        | ContentError::RecordIdentity
        | ContentError::CollectionTooLarge { .. }
        | ContentError::SelfDependency { .. }
        | ContentError::MissingDependency { .. }
        | ContentError::CollectionLimit { .. }
        | ContentError::RecordOffsetPastEnd { .. }
        | ContentError::AnswerDisplayTooLarge => ErrorClass::BadRequest,
        ContentError::Damaged { .. }
        | ContentError::SavedAnswerCount { .. }
        | ContentError::Storage(_) => ErrorClass::Internal,
    }
}

#[must_use]
pub(crate) const fn input(failure: &InputFailure) -> ErrorClass {
    match failure {
        InputFailure::Rejected(rejected) => send_rejected(rejected),
        InputFailure::Write(write) => self::write(write),
        InputFailure::ImageMetadata { .. } => ErrorClass::Internal,
    }
}

#[must_use]
pub(crate) const fn answer(failure: &AnswerFailure) -> ErrorClass {
    match failure {
        AnswerFailure::NotFound => ErrorClass::NotFound,
        AnswerFailure::Refused(_) => ErrorClass::Conflict,
        AnswerFailure::Content(content) => self::content(content),
        AnswerFailure::Input(input) => self::input(input),
    }
}

#[must_use]
pub const fn setting(failure: &SettingFailure) -> ErrorClass {
    match failure {
        SettingFailure::Write(write) => self::write(write),
        SettingFailure::McpTargetMissing { .. } => ErrorClass::NotFound,
        SettingFailure::McpChanged { .. }
        | SettingFailure::McpNameTaken { .. }
        | SettingFailure::McpUnnamed
        | SettingFailure::Customizations(_) => ErrorClass::Conflict,
        SettingFailure::RevisionExhausted => ErrorClass::Internal,
    }
}

#[must_use]
pub const fn update_thread(failure: &UpdateThreadFailure) -> ErrorClass {
    match failure {
        UpdateThreadFailure::InvalidConfig => ErrorClass::BadRequest,
        UpdateThreadFailure::Storage(_) => ErrorClass::Internal,
    }
}

#[must_use]
pub const fn create_thread(error: &CreateThreadError) -> ErrorClass {
    match error {
        CreateThreadError::ProjectNotFound { .. } | CreateThreadError::ParentNotFound { .. } => {
            ErrorClass::NotFound
        }
        CreateThreadError::ProjectRemoved { .. } => ErrorClass::Conflict,
        CreateThreadError::AgentWithoutParent { .. }
        | CreateThreadError::AgentNotInParent { .. }
        | CreateThreadError::InvalidConfig { .. } => ErrorClass::BadRequest,
        CreateThreadError::Storage { .. } => ErrorClass::Internal,
    }
}

#[must_use]
pub(crate) const fn spawn(failure: &SpawnFailure) -> ErrorClass {
    match failure {
        SpawnFailure::Thread(thread) => create_thread(thread),
        SpawnFailure::Input(input) => self::input(input),
    }
}

#[must_use]
pub const fn grep(failure: &GrepFailure) -> ErrorClass {
    match failure {
        GrepFailure::Pattern(_) => ErrorClass::BadRequest,
        GrepFailure::Content(content) => self::content(content),
    }
}

#[must_use]
pub const fn create_project(failure: &CreateProjectFailure) -> ErrorClass {
    match failure {
        CreateProjectFailure::Storage(_) => ErrorClass::Internal,
        CreateProjectFailure::TooMany { .. }
        | CreateProjectFailure::PathTooLong { .. }
        | CreateProjectFailure::NameTooLong { .. } => ErrorClass::Conflict,
    }
}

#[must_use]
pub const fn coordination(failure: &forge_store::coordination::CoordinationFailure) -> ErrorClass {
    use forge_store::coordination::CoordinationFailure as Failure;
    match failure {
        Failure::NotFound { .. } | Failure::RecordGone | Failure::CallerGone { .. } => {
            ErrorClass::NotFound
        }
        Failure::Stale(_)
        | Failure::RecordOfAnother
        | Failure::UseTransition
        | Failure::NotClaimant
        | Failure::NotRecipient
        | Failure::MessageSettled
        | Failure::NotSender
        | Failure::CannotFail
        | Failure::NotReceiptSender
        | Failure::NotSubscriber
        | Failure::WatchNotOwner
        | Failure::TransitionInvalid
        | Failure::DependencyUnpublished { .. }
        | Failure::DependencyCycle
        | Failure::WatchTargetUnpublished => ErrorClass::Conflict,
        Failure::PathInvalid
        | Failure::PathNotAbsolute
        | Failure::PathEscapes
        | Failure::OutsideWorktree
        | Failure::NamedResourceInvalid
        | Failure::RecordTooLarge
        | Failure::TooManyDependencies
        | Failure::WorkInvalid
        | Failure::WorkMetadataTooLarge
        | Failure::ClaimInvalid
        | Failure::ClaimLifetime
        | Failure::RecipientInvalid
        | Failure::MessageInvalid
        | Failure::WatchInvalid
        | Failure::RenewalInvalid
        | Failure::ReplyTooLong => ErrorClass::BadRequest,
        Failure::Storage(_) => ErrorClass::Internal,
    }
}

use crate::extensions::{
    AdoptSkillError, DiscardSkillError, EnabledSkillsError, GlobalPromptError, InstallSkillError,
    MarketplaceInstallError, MarketplaceSearchError, PROMPT_MAX_BYTES, ReconcileSkillsError,
    ResolveGlobalPromptError, SaveGlobalPromptError, SetSkillEnabledError, SkillCatalogError,
};
use crate::mcp_auth::{McpSignInFinishError, McpSignInStartError, McpStoreError};
use crate::operation::{
    AnswerError, AnswerRefusal, CancelError, ClearGoalError, ClearGoalRefusal, ContinueError,
    ContinueRefusal, SendError, UpdateError,
};
use crate::resources::ResourceError;
use crate::runtime::{ControlError, EditorReplyError, PeerError};
use crate::scratch::ThreadFolderError;
use artisan_api::{
    AgentFailure, AnswerRefusal as WireRefusal, ContentId, ExtensionFailure, Failure, ItemId,
    Limited, McpFailure, ProjectFailure, ResourceFailure, RunFailure, SettingFailure as Setting,
    ThreadFailure, ThreadId,
};
use forge_store::{
    AnswerFailure, AnswerRefused, BlobFailure, BlobRejected, ContentError, CreateProjectFailure,
    CreateThreadError, InputFailure, SendRejected, SettingFailure, SourceIndexError,
    ThreadsPageFailure, UpdateThreadFailure, WriteFailure,
};

fn wide(count: usize) -> u64 {
    u64::try_from(count).expect("invariant: a usize count fits in u64")
}

fn thread_missing(thread: &ThreadId) -> Failure {
    Failure::Thread(ThreadFailure::NotFound {
        thread: thread.clone(),
    })
}

fn item_missing(item: &ItemId) -> Failure {
    Failure::Thread(ThreadFailure::ItemNotFound { item: item.clone() })
}

fn too_large(subject: Limited, actual: u64, limit: u64) -> Failure {
    Failure::Resource(ResourceFailure::TooLarge {
        subject,
        actual: Some(actual),
        limit,
    })
}

fn over_limit(subject: Limited, limit: u64) -> Failure {
    Failure::Resource(ResourceFailure::TooLarge {
        subject,
        actual: None,
        limit,
    })
}

fn content_missing(content: &ContentId) -> Failure {
    Failure::Resource(ResourceFailure::ContentNotFound {
        content: content.clone(),
    })
}

fn past_end(content: &ContentId, offset: u64, total: u64) -> Failure {
    Failure::Resource(ResourceFailure::OffsetPastEnd {
        content: content.clone(),
        offset,
        total,
    })
}

fn range_limit(limit: u32, max: u32) -> Failure {
    Failure::Resource(ResourceFailure::RangeLimit {
        limit: u64::from(limit),
        min: 1,
        max: u64::from(max),
    })
}

#[must_use]
pub fn write(failure: &WriteFailure) -> Failure {
    match failure {
        WriteFailure::ThreadNotFound { thread } => thread_missing(thread),
        WriteFailure::SettingsFull { key, bytes } => Failure::Setting(Setting::Full {
            key: key.clone(),
            bytes: *bytes,
            limit: forge_store::SETTINGS_BYTES,
        }),
        WriteFailure::Content(content) => self::content(content),
        WriteFailure::ItemMoved { .. }
        | WriteFailure::NotAnInput { .. }
        | WriteFailure::SettingNotAnObject { .. }
        | WriteFailure::RunMoved(_)
        | WriteFailure::Storage(_) => Failure::Internal,
    }
}

#[must_use]
pub fn content(error: &ContentError) -> Failure {
    match error {
        ContentError::TextTooLarge { bytes } => {
            too_large(Limited::Text, *bytes, forge_store::MAX_SOURCE_BYTES)
        }
        ContentError::FieldTooLong { bytes, .. } => too_large(
            Limited::Field,
            wide(*bytes),
            wide(forge_store::MAX_FIELD_BYTES),
        ),
        ContentError::TooManyEntries { entries, .. } => too_large(
            Limited::FieldEntries,
            wide(*entries),
            wide(forge_store::MAX_FIELD_ENTRIES),
        ),
        ContentError::MetadataTooLarge { bytes } => {
            too_large(Limited::Metadata, *bytes, wide(forge_store::MAX_ITEM_BYTES))
        }
        ContentError::DataTooLarge { bytes } => {
            too_large(Limited::ItemData, *bytes, forge_store::MAX_SOURCE_BYTES)
        }
        ContentError::RecordTooLarge { bytes } => too_large(
            Limited::Record,
            wide(*bytes),
            wide(forge_content::INLINE_COLLECTION_BYTES),
        ),
        ContentError::CollectionTooLarge { bytes } => too_large(
            Limited::Collection,
            *bytes,
            forge_store::MAX_COLLECTION_BYTES,
        ),
        ContentError::AnswerDisplayTooLarge => {
            over_limit(Limited::AnswerDisplay, forge_store::MAX_SOURCE_BYTES)
        }
        ContentError::InvalidIdentity { content } => {
            Failure::Resource(ResourceFailure::InvalidContent {
                content: content.clone(),
            })
        }
        ContentError::UnknownReference { content } | ContentError::Missing { content } => {
            content_missing(content)
        }
        ContentError::RangeLimit { limit } => Failure::Resource(ResourceFailure::RangeLimit {
            limit: u64::from(*limit),
            min: 4,
            max: u64::from(artisan_api::CONTENT_RANGE_BYTES),
        }),
        ContentError::OffsetPastEnd {
            content,
            offset,
            total,
        }
        | ContentError::RecordOffsetPastEnd {
            content,
            offset,
            total,
        } => past_end(content, *offset, *total),
        ContentError::RecordIdentity => Failure::Resource(ResourceFailure::RecordIdentity),
        ContentError::SelfDependency { content } => {
            Failure::Resource(ResourceFailure::SelfDependency {
                content: content.clone(),
            })
        }
        ContentError::MissingDependency { content } => {
            Failure::Resource(ResourceFailure::MissingDependency {
                content: content.clone(),
            })
        }
        ContentError::CollectionLimit { limit } => {
            range_limit(*limit, artisan_api::COLLECTION_RANGE_ENTRIES)
        }
        ContentError::Refused(refused) => answer_refused(None, refused),
        ContentError::Damaged { .. }
        | ContentError::SavedAnswerCount { .. }
        | ContentError::Storage(_) => Failure::Internal,
    }
}

fn answer_refused(item: Option<&ItemId>, refused: &AnswerRefused) -> Failure {
    let (item, reason) = match refused {
        AnswerRefused::Invalid(_)
        | AnswerRefused::StoredChoices
        | AnswerRefused::UnknownChoice
        | AnswerRefused::CountMismatch => (
            item.cloned(),
            WireRefusal::Invalid {
                detail: refused.to_string(),
            },
        ),
        AnswerRefused::EngineStopped => (item.cloned(), WireRefusal::EngineStopped),
        AnswerRefused::AnsweredDifferently => (item.cloned(), WireRefusal::AnsweredDifferently),
        AnswerRefused::NotWaiting => (item.cloned(), WireRefusal::NotWaiting),
        AnswerRefused::NotAQuestion { item } => (Some(item.clone()), WireRefusal::NotAQuestion),
    };
    Failure::Run(RunFailure::AnswerRefused { item, reason })
}

fn send_rejected(rejected: &SendRejected) -> Failure {
    match rejected {
        SendRejected::ThreadNotFound(thread) => thread_missing(thread),
        SendRejected::IdConflict(item) => {
            Failure::Run(RunFailure::MessageIdTaken { item: item.clone() })
        }
        SendRejected::TextTooLarge { bytes } => Failure::Run(RunFailure::TextTooLarge {
            bytes: *bytes,
            limit: forge_store::MAX_MESSAGE_TEXT_BYTES,
        }),
        SendRejected::TooManyImages { count } => Failure::Run(RunFailure::TooManyImages {
            count: wide(*count),
            limit: wide(forge_store::MAX_MESSAGE_IMAGES),
        }),
        SendRejected::ImageTooLarge { blob, bytes } => Failure::Run(RunFailure::ImageTooLarge {
            blob: blob.clone(),
            bytes: *bytes,
            limit: wide(forge_store::MAX_BLOB_BYTES),
        }),
        SendRejected::ImagesTooLarge { bytes } => Failure::Run(RunFailure::ImagesTooLarge {
            bytes: *bytes,
            limit: forge_store::MAX_MESSAGE_IMAGE_BYTES,
        }),
        SendRejected::MissingBlob(blob) => {
            Failure::Run(RunFailure::ImageMissing { blob: blob.clone() })
        }
        SendRejected::InvalidConfig => Failure::Run(RunFailure::InvalidConfig),
    }
}

#[must_use]
fn input(failure: &InputFailure) -> Failure {
    match failure {
        InputFailure::Rejected(rejected) => send_rejected(rejected),
        InputFailure::Write(write) => self::write(write),
        InputFailure::ImageMetadata { .. } => Failure::Internal,
    }
}

fn answer(item: &ItemId, failure: &AnswerFailure) -> Failure {
    match failure {
        AnswerFailure::NotFound => item_missing(item),
        AnswerFailure::Refused(refused) => answer_refused(Some(item), refused),
        AnswerFailure::Content(content) => self::content(content),
        AnswerFailure::Input(input) => self::input(input),
    }
}

#[must_use]
pub fn setting(failure: &SettingFailure) -> Failure {
    match failure {
        SettingFailure::Write(write) => self::write(write),
        SettingFailure::Customizations(source) => {
            Failure::Setting(Setting::InvalidCustomizations {
                detail: diagnostics::chain(source),
            })
        }
        SettingFailure::McpChanged { server } => Failure::Mcp(McpFailure::Changed {
            server: server.clone(),
        }),
        SettingFailure::McpUnnamed => Failure::Mcp(McpFailure::Unnamed),
        SettingFailure::McpTargetMissing { server } => Failure::Mcp(McpFailure::TargetMissing {
            server: server.clone(),
        }),
        SettingFailure::McpNameTaken { server } => Failure::Mcp(McpFailure::NameTaken {
            server: server.clone(),
        }),
        SettingFailure::RevisionExhausted => Failure::Internal,
    }
}

#[must_use]
pub fn draft(scope: &str, failure: &SettingFailure) -> Failure {
    match failure {
        SettingFailure::Write(WriteFailure::SettingsFull { bytes, .. }) => {
            Failure::Draft(artisan_api::DraftFailure::Full {
                scope: scope.to_owned(),
                bytes: *bytes,
                limit: forge_store::SETTINGS_BYTES,
            })
        }
        SettingFailure::Write(
            WriteFailure::ThreadNotFound { .. }
            | WriteFailure::ItemMoved { .. }
            | WriteFailure::NotAnInput { .. }
            | WriteFailure::SettingNotAnObject { .. }
            | WriteFailure::RunMoved(_)
            | WriteFailure::Content(_)
            | WriteFailure::Storage(_),
        )
        | SettingFailure::Customizations(_)
        | SettingFailure::RevisionExhausted
        | SettingFailure::McpChanged { .. }
        | SettingFailure::McpUnnamed
        | SettingFailure::McpTargetMissing { .. }
        | SettingFailure::McpNameTaken { .. } => setting(failure),
    }
}

#[must_use]
fn update_thread(thread: &ThreadId, failure: &UpdateThreadFailure) -> Failure {
    match failure {
        UpdateThreadFailure::InvalidConfig => Failure::Thread(ThreadFailure::InvalidConfig {
            thread: thread.clone(),
        }),
        UpdateThreadFailure::Storage(_) => Failure::Internal,
    }
}

#[must_use]
pub fn create_thread(error: &CreateThreadError) -> Failure {
    match error {
        CreateThreadError::ProjectNotFound { project, .. } => {
            Failure::Project(ProjectFailure::NotFound {
                project: project.clone(),
            })
        }
        CreateThreadError::ProjectRemoved { project, .. } => {
            Failure::Project(ProjectFailure::Removed {
                project: project.clone(),
            })
        }
        CreateThreadError::ParentNotFound { thread, parent } => {
            Failure::Thread(ThreadFailure::ParentNotFound {
                thread: thread.clone(),
                parent: parent.clone(),
            })
        }
        CreateThreadError::AgentWithoutParent { thread, agent } => {
            Failure::Agent(AgentFailure::WithoutParent {
                thread: thread.clone(),
                agent: agent.clone(),
            })
        }
        CreateThreadError::AgentNotInParent {
            thread,
            parent,
            agent,
        } => Failure::Agent(AgentFailure::NotInParent {
            thread: thread.clone(),
            parent: parent.clone(),
            agent: agent.clone(),
        }),
        CreateThreadError::InvalidConfig { thread } => {
            Failure::Thread(ThreadFailure::InvalidConfig {
                thread: thread.clone(),
            })
        }
        CreateThreadError::Storage { .. } => Failure::Internal,
    }
}

#[must_use]
pub fn create_project(failure: &CreateProjectFailure) -> Failure {
    match failure {
        CreateProjectFailure::TooMany { limit } => Failure::Project(ProjectFailure::TooMany {
            limit: wide(*limit),
        }),
        CreateProjectFailure::PathTooLong { bytes } => {
            Failure::Project(ProjectFailure::PathTooLong {
                bytes: wide(*bytes),
                limit: wide(forge_store::MAX_PROJECT_PATH_BYTES),
            })
        }
        CreateProjectFailure::NameTooLong { bytes } => {
            Failure::Project(ProjectFailure::NameTooLong {
                bytes: wide(*bytes),
                limit: wide(forge_store::MAX_PROJECT_NAME_BYTES),
            })
        }
        CreateProjectFailure::Storage(_) => Failure::Internal,
    }
}

#[must_use]
pub fn threads_page(failure: &ThreadsPageFailure) -> Failure {
    match failure {
        ThreadsPageFailure::SearchTooLong { bytes } => {
            Failure::Thread(ThreadFailure::SearchTooLong {
                bytes: wide(*bytes),
                limit: wide(forge_store::MAX_SEARCH_BYTES),
            })
        }
        ThreadsPageFailure::Storage(_) => Failure::Internal,
    }
}

#[must_use]
pub fn source_index(error: &SourceIndexError) -> Failure {
    match error {
        SourceIndexError::InvalidIdentity { source_id } => {
            Failure::Resource(ResourceFailure::InvalidContent {
                content: source_id.clone(),
            })
        }
        SourceIndexError::PageLimit { limit } => {
            range_limit(*limit, artisan_api::SOURCE_INDEX_PAGE_LEAVES)
        }
        SourceIndexError::OffsetPastEnd {
            source_id,
            offset,
            leaves,
        } => past_end(source_id, *offset, *leaves),
        SourceIndexError::Markdown { source_id, source } => {
            Failure::Resource(ResourceFailure::Markdown {
                content: source_id.clone(),
                detail: diagnostics::chain(source),
            })
        }
        SourceIndexError::MetadataTooLarge { .. } => {
            over_limit(Limited::SourceIndex, wide(forge_store::MAX_INDEX_BYTES))
        }
        SourceIndexError::Content(content) => self::content(content),
        SourceIndexError::IdentityMismatch { .. }
        | SourceIndexError::MissingRecord { .. }
        | SourceIndexError::SelfOwned { .. }
        | SourceIndexError::TopologyGap { .. }
        | SourceIndexError::TopologyOrdinals { .. }
        | SourceIndexError::Storage(_) => Failure::Internal,
    }
}

#[must_use]
pub fn blob(failure: &BlobFailure) -> Failure {
    match failure {
        BlobFailure::Rejected(BlobRejected::NotWebp) => Failure::Resource(ResourceFailure::NotWebp),
        BlobFailure::Rejected(BlobRejected::TooLarge { bytes }) => too_large(
            Limited::Upload,
            wide(*bytes),
            wide(forge_store::MAX_BLOB_BYTES),
        ),
        BlobFailure::TooManyPixels { width, height } => {
            Failure::Resource(ResourceFailure::TooManyPixels {
                width: *width,
                height: *height,
                limit: forge_store::MAX_IMAGE_PIXELS,
            })
        }
        BlobFailure::Image(_)
        | BlobFailure::WebPEncoder
        | BlobFailure::WebPEncode { .. }
        | BlobFailure::Io { .. }
        | BlobFailure::Storage(_) => Failure::Internal,
    }
}

impl PeerError {
    #[must_use]
    fn wire(&self) -> Failure {
        match self {
            Self::Rejected { failure, .. } => failure.wire(),
            Self::Client { .. } | Self::Send { .. } | Self::Decode { .. } => Failure::Internal,
        }
    }
}

impl SendError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::ThreadNotFound { thread } => thread_missing(thread),
            Self::Command { thread, engine, .. } => Failure::Run(RunFailure::CommandUnsupported {
                thread: thread.clone(),
                engine: *engine,
            }),
            Self::Store(store) => input(&store.cause),
            Self::Peer { source, .. } => source.wire(),
            Self::ReadThread(_) | Self::Route { .. } | Self::Abandoned { .. } => Failure::Internal,
        }
    }
}

impl AnswerError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::ItemNotFound { item } => item_missing(item),
            Self::Refused { item, reason } => Failure::Run(RunFailure::AnswerRefused {
                item: Some(item.clone()),
                reason: match reason {
                    AnswerRefusal::NotAnApproval => WireRefusal::NotAnApproval,
                    AnswerRefusal::DecisionNotOffered => WireRefusal::DecisionNotOffered,
                    AnswerRefusal::NotWaiting => WireRefusal::NotWaiting,
                    AnswerRefusal::RunEnded => WireRefusal::RunEnded,
                },
            }),
            Self::Validate(validate) => answer(&validate.item, &validate.cause),
            Self::Save(save) => answer(&save.item, &save.cause),
            Self::Peer { source, .. } => source.wire(),
            Self::ReadItem(_)
            | Self::NativeId(_)
            | Self::NoEngineId { .. }
            | Self::Route { .. } => Failure::Internal,
        }
    }
}

impl ContinueError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::Refused { thread, reason } => Failure::Run(match reason {
                ContinueRefusal::Working => RunFailure::AlreadyWorking {
                    thread: thread.clone(),
                },
                ContinueRefusal::NothingStopped => RunFailure::NothingToContinue {
                    thread: thread.clone(),
                },
            }),
            Self::LatestRun(_) | Self::Restore(_) | Self::Request(_) => Failure::Internal,
        }
    }
}

impl UpdateError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::Store(store) => update_thread(&store.thread, &store.cause),
            Self::Peer { source, .. } => source.wire(),
            Self::ReadThread(_) | Self::Route { .. } => Failure::Internal,
        }
    }
}

impl ClearGoalError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::ThreadNotFound { thread } => thread_missing(thread),
            Self::Refused { thread, reason } => Failure::Run(match reason {
                ClearGoalRefusal::NoGoal => RunFailure::NoGoal {
                    thread: thread.clone(),
                },
                ClearGoalRefusal::Ending => RunFailure::Ending {
                    thread: thread.clone(),
                },
                ClearGoalRefusal::NoSession { engine } => RunFailure::NoSession {
                    thread: thread.clone(),
                    engine: *engine,
                },
            }),
            Self::Send { source, .. } => source.wire(),
            Self::Peer { source, .. } => source.wire(),
            Self::ReadThread(_)
            | Self::Inspector(_)
            | Self::LatestRun(_)
            | Self::Prepare { .. }
            | Self::Engine { .. }
            | Self::Notice(_)
            | Self::Route { .. } => Failure::Internal,
        }
    }
}

impl CancelError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::Peer { source, .. } => source.wire(),
            Self::Tree(_) | Self::Inputs(_) | Self::Unpark(_) | Self::Route { .. } => {
                Failure::Internal
            }
        }
    }
}

impl EditorReplyError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::Peer { source, .. } => source.wire(),
            Self::Peers { .. } => Failure::Internal,
        }
    }
}

impl ControlError {
    #[must_use]
    pub(crate) fn wire(&self) -> Failure {
        match self {
            Self::Accept(error) => error.wire(),
            Self::Answer(error) => error.wire(),
            Self::Cancel(error) => error.wire(),
            Self::Update(error) => error.wire(),
            Self::ClearGoal(error) => error.wire(),
            Self::Unmanaged | Self::Activate(_) | Self::Encode { .. } | Self::Lifecycle { .. } => {
                Failure::Internal
            }
        }
    }
}

impl ThreadFolderError {
    const fn wire(&self) -> Failure {
        match self {
            Self::Task { .. }
            | Self::Scratch { .. }
            | Self::ReadProject { .. }
            | Self::NoProject { .. } => Failure::Internal,
        }
    }
}

impl ResourceError {
    #[must_use]
    pub fn wire(&self, requested: &str) -> Failure {
        let path = |path: &std::path::Path| path.display().to_string();
        match self {
            Self::Missing => Failure::Resource(ResourceFailure::FileNotFound {
                path: requested.to_owned(),
            }),
            Self::InvalidUrl { path, .. } => {
                Failure::Resource(ResourceFailure::InvalidUrl { path: path.clone() })
            }
            Self::OtherDistribution { distribution } => {
                Failure::Resource(ResourceFailure::OtherDistribution {
                    path: requested.to_owned(),
                    distribution: distribution.clone(),
                })
            }
            Self::NoHome => Failure::Resource(ResourceFailure::NoHome),
            Self::NotRegular { path: file } => {
                Failure::Resource(ResourceFailure::NotAFile { path: path(file) })
            }
            Self::Outside { path: file } => {
                Failure::Resource(ResourceFailure::Outside { path: path(file) })
            }
            Self::Refused { path: file, .. } => {
                Failure::Resource(ResourceFailure::Refused { path: path(file) })
            }
            Self::Linked { path: file, .. } => {
                Failure::Resource(ResourceFailure::Linked { path: path(file) })
            }
            Self::Folder(source) => source.wire(),
            Self::Read { .. } | Self::Root { .. } | Self::Open { .. } | Self::Task { .. } => {
                Failure::Internal
            }
        }
    }
}

impl McpStoreError {
    #[must_use]
    pub const fn wire(&self) -> Failure {
        match self {
            Self::Servers(_)
            | Self::ReadStatuses(_)
            | Self::EncodeStatuses(_)
            | Self::RecordStatuses(_)
            | Self::RecordAuth { .. }
            | Self::ReadSecret { .. }
            | Self::SaveSecret { .. }
            | Self::ForgetSecret { .. }
            | Self::Encoding { .. } => Failure::Internal,
        }
    }
}

impl McpSignInStartError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        let server = |server: &String| server.clone();
        Failure::Mcp(match self {
            Self::UnknownServer { server: name } => McpFailure::UnknownServer {
                server: server(name),
            },
            Self::NotHttp { server: name } => McpFailure::NotHttp {
                server: server(name),
            },
            Self::Probe {
                server: name,
                source,
            } => McpFailure::Probe {
                server: server(name),
                detail: diagnostics::chain(source),
            },
            Self::Open { server: name } => McpFailure::Open {
                server: server(name),
            },
            Self::NeedsToken { server: name } => McpFailure::NeedsToken {
                server: server(name),
            },
            Self::Registration {
                server: name,
                source,
            } => McpFailure::Registration {
                server: server(name),
                detail: diagnostics::chain(source),
            },
            Self::Removed { server: name } => McpFailure::ChangedDuringSignIn {
                server: server(name),
            },
            Self::Random { .. } => return Failure::Internal,
            Self::Store(store) => return store.wire(),
        })
    }
}

impl McpSignInFinishError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        Failure::Mcp(match self {
            Self::Expired => McpFailure::SignInExpired,
            Self::SignedOut { server } => McpFailure::SignedOutDuringSignIn {
                server: server.clone(),
            },
            Self::Exchange { server, source } => McpFailure::Exchange {
                server: server.clone(),
                detail: diagnostics::chain(source),
            },
            Self::Removed { server } => McpFailure::ChangedDuringSignIn {
                server: server.clone(),
            },
            Self::Store(store) => return store.wire(),
        })
    }
}

fn skill(skill: &str, source: &(dyn std::error::Error + 'static)) -> Failure {
    Failure::Extension(ExtensionFailure::Skill {
        skill: skill.to_owned(),
        detail: diagnostics::chain(source),
    })
}

impl SkillCatalogError {
    #[must_use]
    pub const fn wire(&self) -> Failure {
        match self {
            Self::Customisation(_) | Self::Homes(_) | Self::Read(_) => Failure::Internal,
        }
    }
}

impl EnabledSkillsError {
    #[must_use]
    pub const fn wire(&self) -> Failure {
        match self {
            Self::Customisation(_) | Self::Homes(_) | Self::Read(_) => Failure::Internal,
        }
    }
}

impl ReconcileSkillsError {
    const fn wire(&self) -> Failure {
        match self {
            Self::Customisation(_) | Self::Homes(_) | Self::Reconcile(_) => Failure::Internal,
        }
    }
}

impl AdoptSkillError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::Adopt {
                skill: name,
                source,
            } => skill(name, &**source),
            Self::Reconcile { source, .. } => source.wire(),
            Self::Bump { source, .. } => setting(&source.cause),
            Self::Catalog { source, .. } => source.wire(),
            Self::Customisation { .. } | Self::Homes { .. } => Failure::Internal,
        }
    }
}

impl DiscardSkillError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::Discard {
                skill: name,
                source,
            } => skill(name, source),
            Self::Bump { source, .. } => setting(&source.cause),
            Self::Catalog { source, .. } => source.wire(),
            Self::Customisation { .. } | Self::Homes { .. } => Failure::Internal,
        }
    }
}

impl InstallSkillError {
    fn wire(&self, listing: &str) -> Failure {
        match self {
            Self::Install { source, .. } => Failure::Extension(ExtensionFailure::Install {
                listing: listing.to_owned(),
                detail: diagnostics::chain(source),
            }),
            Self::Enable { source, .. } => write(&source.cause),
            Self::Reconcile { source, .. } => source.wire(),
            Self::Bump { source, .. } => setting(&source.cause),
            Self::Customisation { .. } | Self::Homes { .. } => Failure::Internal,
        }
    }
}

impl MarketplaceInstallError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::Fetch { listing, source } => Failure::Extension(ExtensionFailure::Install {
                listing: listing.clone(),
                detail: diagnostics::chain(&**source),
            }),
            Self::Install { listing, source } => source.wire(listing),
            Self::Catalog { source, .. } => source.wire(),
            Self::Staging { .. } | Self::StagingTask { .. } => Failure::Internal,
        }
    }
}

impl MarketplaceSearchError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::Search { query, source } => Failure::Extension(ExtensionFailure::Search {
                query: query.clone(),
                detail: diagnostics::chain(source),
            }),
        }
    }
}

impl GlobalPromptError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::TakeOver(source) => setting(&source.cause),
            Self::Read(_) | Self::Candidates(_) | Self::Version(_) | Self::Engines(_) => {
                Failure::Internal
            }
        }
    }
}

impl SaveGlobalPromptError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::TooLong { bytes, .. } => Failure::Extension(ExtensionFailure::PromptTooLong {
                bytes: wide(*bytes),
                limit: wide(PROMPT_MAX_BYTES),
            }),
            Self::Stale { base, current, .. } => {
                Failure::Extension(ExtensionFailure::PromptChanged {
                    base: *base,
                    current: *current,
                })
            }
            Self::Save { source, .. } => write(&source.cause),
            Self::Reconcile(source) => source.wire(),
        }
    }
}

impl ResolveGlobalPromptError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::NotDrifted { engine, .. } => {
                Failure::Extension(ExtensionFailure::PromptNotDrifted { engine: *engine })
            }
            Self::NoCandidate { candidate, .. } => {
                Failure::Extension(ExtensionFailure::PromptCandidateMissing {
                    candidate: candidate.clone(),
                })
            }
            Self::Changed {
                version, current, ..
            } => Failure::Extension(ExtensionFailure::PromptChanged {
                base: *version,
                current: *current,
            }),
            Self::Save { source, .. } => write(&source.cause),
            Self::Bump(source) => setting(&source.cause),
            Self::Current(source) | Self::Reconcile(source) => source.wire(),
        }
    }
}

impl SetSkillEnabledError {
    #[must_use]
    pub fn wire(&self) -> Failure {
        match self {
            Self::Unmanaged { skill, .. } => Failure::Extension(ExtensionFailure::Unmanaged {
                skill: skill.clone(),
            }),
            Self::Save { source, .. } => write(&source.cause),
            Self::Catalog { source, .. } => source.wire(),
            Self::Reconcile { source, .. } => source.wire(),
        }
    }
}

#[must_use]
pub fn message(error: &(dyn std::error::Error + 'static), failure: &Failure) -> String {
    match failure {
        Failure::Internal => diagnostics::chain(error),
        Failure::Legacy { .. } => std::iter::successors(Some(error), |cause| cause.source())
            .find_map(|cause| cause.downcast_ref::<crate::runtime::RemoteFailure>())
            .map_or_else(
                || diagnostics::chain(error),
                |remote| remote.message.clone(),
            ),
        Failure::Thread(_)
        | Failure::Run(_)
        | Failure::Draft(_)
        | Failure::Setting(_)
        | Failure::Project(_)
        | Failure::Mcp(_)
        | Failure::Agent(_)
        | Failure::Auth(_)
        | Failure::Resource(_)
        | Failure::Extension(_)
        | Failure::Engine(_) => failure.to_string(),
    }
}

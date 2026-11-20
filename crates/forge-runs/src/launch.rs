use artisan_api::{EngineId, Run};

use crate::Hub;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionId(String);

impl SessionId {
    fn of(run: &Run) -> Option<Self> {
        run.session_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .map(|id| Self(id.to_owned()))
    }

    fn on_claude_ai(&self) -> bool {
        self.0.starts_with(artisan_api::CLAUDE_AI_SESSION_PREFIX)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaudeTransport {
    Code,
    Chat,
    ChatSignedOut,
}

impl ClaudeTransport {
    fn carries(self, session: &SessionId) -> bool {
        match self {
            Self::Chat => session.on_claude_ai(),
            Self::Code | Self::ChatSignedOut => !session.on_claude_ai(),
        }
    }

    const fn option(self) -> Option<&'static str> {
        match self {
            Self::Code => None,
            Self::Chat => Some(forge_engines::CLAUDE_AI_TRANSPORT),
            Self::ChatSignedOut => Some(forge_engines::CLAUDE_AI_UNAVAILABLE),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LaunchSession {
    Claude {
        transport: ClaudeTransport,
        resume: Option<SessionId>,
    },
    Codex {
        resume: Option<SessionId>,
    },
    OpenCode {
        resume: Option<SessionId>,
    },
}

pub(crate) enum Branch {
    Anywhere,
    OffClaudeAi,
}

impl LaunchSession {
    pub(crate) async fn current(
        hub: &Hub,
        engine: EngineId,
        previous: Option<&Run>,
    ) -> Result<Self, forge_store::ReadSettingError> {
        let session = previous
            .filter(|run| run.engine == engine)
            .and_then(SessionId::of);
        Ok(match engine {
            EngineId::Claude => {
                let transport = claude_transport(hub).await?;
                Self::Claude {
                    transport,
                    resume: session.filter(|session| transport.carries(session)),
                }
            }
            EngineId::Codex => Self::Codex { resume: session },
            EngineId::OpenCode => Self::OpenCode { resume: session },
        })
    }

    pub(crate) async fn fresh(
        hub: &Hub,
        engine: EngineId,
    ) -> Result<Self, forge_store::ReadSettingError> {
        Self::current(hub, engine, None).await
    }

    pub(crate) async fn native(
        hub: &Hub,
        engine: EngineId,
        previous: Option<&Run>,
        branch: Branch,
    ) -> Result<Self, forge_store::ReadSettingError> {
        let session = previous
            .filter(|run| run.engine == engine)
            .and_then(SessionId::of);
        match (engine, session) {
            (EngineId::Claude, Some(session)) if !session.on_claude_ai() => Ok(Self::Claude {
                transport: ClaudeTransport::Code,
                resume: Some(session),
            }),
            (EngineId::Claude, Some(session))
                if matches!(branch, Branch::Anywhere) && signed_in_to_claude_ai(hub) =>
            {
                Ok(Self::Claude {
                    transport: ClaudeTransport::Chat,
                    resume: Some(session),
                })
            }
            (EngineId::Claude, _) => Self::fresh(hub, EngineId::Claude).await,
            (EngineId::Codex, session) => Ok(Self::Codex { resume: session }),
            (EngineId::OpenCode, session) => Ok(Self::OpenCode { resume: session }),
        }
    }

    pub(crate) const fn resume(&self) -> Option<&SessionId> {
        match self {
            Self::Claude { resume, .. } | Self::Codex { resume } | Self::OpenCode { resume } => {
                resume.as_ref()
            }
        }
    }

    pub(crate) const fn transport_option(&self) -> Option<&'static str> {
        match self {
            Self::Claude { transport, .. } => transport.option(),
            Self::Codex { .. } | Self::OpenCode { .. } => None,
        }
    }

    pub(crate) const fn on_claude_ai(&self) -> bool {
        matches!(
            self,
            Self::Claude {
                transport: ClaudeTransport::Chat,
                ..
            }
        )
    }
}

async fn claude_transport(hub: &Hub) -> Result<ClaudeTransport, forge_store::ReadSettingError> {
    let setting = hub.store().setting(artisan_api::TRANSPORT_KEY).await?;
    if artisan_api::transport_of(setting.as_ref()) != artisan_api::CLAUDE_AI {
        return Ok(ClaudeTransport::Code);
    }
    Ok(if signed_in_to_claude_ai(hub) {
        ClaudeTransport::Chat
    } else {
        ClaudeTransport::ChatSignedOut
    })
}

fn signed_in_to_claude_ai(hub: &Hub) -> bool {
    forge_engines::claudeai::Session::path(&hub.engines().home(EngineId::Claude)).is_file()
}

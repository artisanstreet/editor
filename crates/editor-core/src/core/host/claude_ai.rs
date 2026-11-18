use super::{HostCompletion, HostData, HostMessage};
use artisan_api::{ClaudeAiSession, ClaudeAiState, EngineCommand, EngineId};
use claude_ai_browser::{RenewError, Step, Ticket};
use tokio_util::task::AbortOnDropHandle;

impl HostData {
    pub(super) fn renew_claude_ai(&mut self) {
        let needs = self.engines.get(&EngineId::Claude).is_some_and(|status| {
            matches!(status.claude_ai, Some(ClaudeAiState::NeedsRenewal { .. }))
        });
        let wanted = needs && claude_ai_browser::can_renew(&self.id.0);
        let host = self.id.clone();
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let step = self
            .claude_ai
            .observe(wanted, std::time::Instant::now(), |ticket| {
                AbortOnDropHandle::new(crate::core::runtime::runtime().spawn(async move {
                    let result = claude_ai_browser::renew(&host.0).await;
                    if sender
                        .send(HostCompletion::ClaudeAiCaptured(ticket, result))
                        .await
                        .is_err()
                    {
                        tracing::debug!(%host, "the host closed before the claude.ai session was renewed");
                    }
                }))
            });
        match step {
            Step::Started(_) => tracing::info!(host = %self.id, "renewing the claude.ai session"),
            Step::Cancelled => {
                tracing::info!(host = %self.id, "stopped renewing the claude.ai session: the host no longer asks for it");
            }
            Step::Unchanged => {}
        }
    }

    pub(super) fn finish_claude_ai_capture(
        &mut self,
        ticket: Ticket,
        result: Result<ClaudeAiSession, RenewError>,
    ) {
        let session = match result {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(host = %self.id, "renewing the claude.ai session failed: {}", diagnostics::chain(&error));
                self.claude_ai.finished(ticket);
                return;
            }
        };
        let client = self.client.clone();
        let host = self.id.clone();
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let handed = self.claude_ai.captured(ticket, move || {
            AbortOnDropHandle::new(crate::core::runtime::runtime().spawn(async move {
                let result = client
                    .engine_action(EngineId::Claude, &EngineCommand::ClaudeAiSignIn { session })
                    .await
                    .map(|_status| ());
                if sender
                    .send(HostCompletion::ClaudeAiHanded(ticket, result))
                    .await
                    .is_err()
                {
                    tracing::debug!(%host, "the host closed before it took the renewed claude.ai session");
                }
            }))
        });
        if !handed {
            tracing::info!(host = %self.id, "discarded a renewed claude.ai session the host no longer asks for");
        }
    }

    pub(super) fn finish_claude_ai_handover(
        &mut self,
        ticket: Ticket,
        result: Result<(), crate::core::client::CallError>,
    ) {
        if let Err(error) = result {
            tracing::warn!(host = %self.id, "the Forge did not take the renewed claude.ai session: {}", ::diagnostics::chain(&error));
        }
        self.claude_ai.finished(ticket);
    }
}

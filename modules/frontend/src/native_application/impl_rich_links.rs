//! Rich-link title resolution handlers for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-4 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    /// Submits one bounded batch of rich-link resolve commands.
    ///
    /// Returns `true` when every URL was accepted by the command bridge.
    /// Backpressure keeps the unsubmitted tail as the next retained effect, so
    /// a retry never re-submits an already-accepted URL.
    pub(super) fn submit_rich_link_requests(
        &mut self,
        mut urls: Vec<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        while let Some(url) = urls.first().cloned() {
            match self.submit_command(NativeTransportCommand::ResolveRichLink { url }) {
                Ok(()) => {
                    urls.remove(0);
                }
                Err(CommandSendError::Busy) => break,
                Err(CommandSendError::Stopped) => {
                    self.set_failure(
                        ServiceFailure {
                            stage: ServiceFailureStage::EventBridge,
                            category: ServiceFailureCategory::ChannelClosed,
                        },
                        cx,
                    );
                    return false;
                }
            }
        }
        if urls.is_empty() {
            return true;
        }
        if let Some(effect) = self.conversation_effects.first_mut() {
            *effect = ConversationHostEffect::RichLinkRequests { urls };
        }
        false
    }

    /// Mirrors one resolved rich-link title into the mounted surface.
    ///
    /// A result for a thread that is no longer mounted is dropped: the title
    /// table is surface-local, so nothing else can consume the value.
    pub(super) fn handle_rich_link_resolved(
        &mut self,
        requested_url: &str,
        page_name: &str,
        expires_at_ms: i64,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        let page_name = SharedString::from(page_name);
        let surface = host.read(cx).surface().clone();
        surface.update(cx, |surface, surface_cx| {
            surface.set_rich_link_title(requested_url, &page_name, expires_at_ms, surface_cx);
        });
        cx.notify();
    }

    /// Records one failed rich-link resolution; the authored label stays.
    pub(super) fn handle_rich_link_failed(&mut self, requested_url: &str, cx: &mut Context<Self>) {
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        let surface = host.read(cx).surface().clone();
        surface.update(cx, |surface, surface_cx| {
            surface.set_rich_link_failure(requested_url, surface_cx);
        });
        cx.notify();
    }
}

//! On-demand history of the open thread.
//!
//! A thread opens on its newest turns, and a settled turn's work rows stay
//! on the Forge. This module reads the rest when the reader gets near it:
//! older turns as the reader scrolls toward the start of what is loaded, and
//! a turn's work rows when its section is open.

use std::collections::BTreeSet;

use artisan_domain::{
    ConversationHistoryPage, ConversationHistoryPart, EarlierTurnMarker, EarlierTurnMarkers,
    QueryTurnCount, TurnId, TurnOrdinal,
};

use crate::conversation_surface::ConversationSurfaceTarget;

use crate::conversation_scene::SceneDisclosure;
use crate::native_transport_service::HISTORY_PAGE_TURNS;

use super::*;

/// Most durable items the loaded turns may hold before older pages stop
/// being read. The scene holds items and activity rows under one bound, so
/// this leaves the running turn's activity its share.
pub(super) const HISTORY_MAX_LOADED_ITEMS: usize =
    crate::conversation_scene::SCENE_MAX_ITEMS * 3 / 4;

/// What is being read for the open thread, and what cannot be.
#[derive(Debug, Default)]
pub(super) struct HistoryPaging {
    /// The reader is near the start of the loaded turns and no page has
    /// been asked for since: the next chance to ask takes it.
    near_start: bool,
    /// A page of older turns was asked for and has not arrived.
    earlier_in_flight: bool,
    /// Ordinal of the oldest loaded turn when the Forge last said nothing
    /// lies before it, or when the loaded turns reached their bound.
    exhausted_before: Option<u64>,
    /// Turns whose held-back work rows are being read.
    turn_work_in_flight: BTreeSet<TurnId>,
    /// Turns whose work rows could not be read; they are left alone until
    /// the thread is opened again or the connection returns.
    turn_work_failed: BTreeSet<TurnId>,
    /// The user messages of the turns before the window the thread opened
    /// on, oldest first. The turn navigator lists the ones still not loaded.
    earlier_markers: Vec<EarlierTurnMarker>,
    /// A user message in a turn that is not loaded, which the reader asked
    /// to jump to: the viewport goes there once its turn arrives.
    jump: Option<artisan_domain::ItemId>,
}

impl NativeApplication {
    /// Forgets every read in flight: the thread changed or the connection
    /// was replaced, so their answers can no longer arrive.
    pub(super) fn reset_history_paging(&mut self) {
        self.history_paging = HistoryPaging::default();
    }

    /// Forgets the reads in flight after the connection was replaced; what
    /// the thread lists for its older turns stays.
    pub(super) fn reset_history_reads(&mut self) {
        let earlier_markers = std::mem::take(&mut self.history_paging.earlier_markers);
        self.history_paging = HistoryPaging {
            earlier_markers,
            ..HistoryPaging::default()
        };
    }

    /// Records the user messages of the open thread's turns that are not
    /// loaded and lists them in the turn navigator.
    pub(super) fn apply_earlier_turn_markers(
        &mut self,
        markers: EarlierTurnMarkers,
        cx: &mut Context<Self>,
    ) {
        if self.selected_thread.as_ref() != Some(&markers.thread_id) {
            return;
        }
        self.history_paging.earlier_markers = markers.markers;
        self.sync_earlier_turn_markers(cx);
    }

    /// Hands the surface the markers of the turns still not loaded.
    fn sync_earlier_turn_markers(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        let floor = host
            .read(cx)
            .canonical_snapshot()
            .and_then(|snapshot| snapshot.turns().first().map(|turn| turn.ordinal));
        let markers: Vec<(artisan_domain::ItemId, String)> = self
            .history_paging
            .earlier_markers
            .iter()
            .filter(|marker| floor.is_none_or(|floor| marker.turn_ordinal < floor))
            .map(|marker| (marker.item_id.clone(), marker.label.clone()))
            .collect();
        let surface = host.read(cx).surface().clone();
        surface.update(cx, |surface, surface_cx| {
            surface.set_earlier_turn_markers(markers, surface_cx);
        });
    }

    /// Starts a jump to a user message whose turn is not loaded: every turn
    /// from its own up to the loaded ones is read, and the viewport goes to
    /// the message once they arrive. Returns whether `target` was such a
    /// message.
    pub(super) fn jump_to_earlier_turn(
        &mut self,
        target: &ConversationSurfaceTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        let ConversationSurfaceTarget::Item(item_id) = target else {
            return false;
        };
        let (Some(thread_id), Some(host)) =
            (self.selected_thread.clone(), self.conversation_host.clone())
        else {
            return false;
        };
        let Some(oldest) = host
            .read(cx)
            .canonical_snapshot()
            .and_then(|snapshot| snapshot.turns().first().map(|turn| turn.ordinal))
        else {
            return false;
        };
        let Some(marker) = self
            .history_paging
            .earlier_markers
            .iter()
            .find(|marker| &marker.item_id == item_id && marker.turn_ordinal < oldest)
        else {
            return false;
        };
        let minimum = marker.turn_ordinal;
        self.history_paging.jump = Some(item_id.clone());
        // A page already in flight lands first; the jump continues from it.
        if self.history_paging.earlier_in_flight {
            return true;
        }
        let Ok(maximum_turn_count) =
            QueryTurnCount::new(u64::from(artisan_domain::CONVERSATION_QUERY_MAX_TURNS))
        else {
            return true;
        };
        let command = NativeTransportCommand::ReadConversationHistory {
            thread_id,
            part: ConversationHistoryPart::EarlierTurns {
                before_turn_ordinal: oldest,
                minimum_turn_ordinal: Some(minimum),
                maximum_turn_count,
            },
        };
        if self.submit_command(command).is_ok() {
            self.history_paging.earlier_in_flight = true;
            self.history_paging.near_start = false;
        } else {
            self.history_paging.jump = None;
        }
        true
    }

    /// Continues a pending jump after a page of older turns was applied, or
    /// gives it up when the turn cannot be loaded.
    fn continue_jump(&mut self, cx: &mut Context<Self>) {
        let Some(item_id) = self.history_paging.jump.clone() else {
            return;
        };
        let Some(host) = self.conversation_host.clone() else {
            self.history_paging.jump = None;
            return;
        };
        let loaded = host.read(cx).canonical_snapshot().is_some_and(|snapshot| {
            snapshot
                .items()
                .iter()
                .any(|item| item.item_id() == &item_id)
        });
        let target = ConversationSurfaceTarget::Item(item_id);
        if loaded {
            self.history_paging.jump = None;
            let surface = host.read(cx).surface().clone();
            surface.update(cx, |surface, surface_cx| {
                let _ = surface.schedule_scroll_target(target, surface_cx);
            });
        } else if !self.jump_to_earlier_turn(&target, cx) {
            self.history_paging.jump = None;
        }
    }

    /// The surface reported the reader near the start of the loaded turns.
    pub(super) fn note_reader_near_start(&mut self, cx: &mut Context<Self>) {
        self.history_paging.near_start = true;
        self.request_earlier_turns(cx);
    }

    /// Reads the page of turns before the oldest loaded one, when the reader
    /// is near the start of what is loaded and more can be held.
    pub(super) fn request_earlier_turns(&mut self, cx: &mut Context<Self>) {
        #[cfg(feature = "flight-recorder")]
        let _trace = artisan_tracing::span!("ui", "request_earlier_turns");
        // A pending jump reads its own, larger page once the one in flight
        // has landed.
        if !self.history_paging.near_start
            || self.history_paging.earlier_in_flight
            || self.history_paging.jump.is_some()
            || !self.selected_history_current()
        {
            return;
        }
        let (Some(thread_id), Some(host)) =
            (self.selected_thread.clone(), self.conversation_host.clone())
        else {
            return;
        };
        let Some(snapshot) = host.read(cx).canonical_snapshot() else {
            return;
        };
        if snapshot.thread_id() != &thread_id {
            return;
        }
        let Some(oldest) = snapshot.turns().first().map(|turn| turn.ordinal) else {
            return;
        };
        // Ordinal zero is a thread's first turn.
        if oldest.get() == 0 || self.history_paging.exhausted_before == Some(oldest.get()) {
            return;
        }
        if snapshot.items().len() >= HISTORY_MAX_LOADED_ITEMS {
            self.history_paging.exhausted_before = Some(oldest.get());
            return;
        }
        let Ok(maximum_turn_count) = QueryTurnCount::new(u64::from(HISTORY_PAGE_TURNS)) else {
            return;
        };
        let command = NativeTransportCommand::ReadConversationHistory {
            thread_id,
            part: ConversationHistoryPart::EarlierTurns {
                before_turn_ordinal: oldest,
                minimum_turn_ordinal: None,
                maximum_turn_count,
            },
        };
        if self.submit_command(command).is_ok() {
            self.history_paging.earlier_in_flight = true;
            self.history_paging.near_start = false;
        }
    }

    /// Reads the held-back work rows of every settled turn whose section is
    /// open, and of every turn that shows its work ungrouped.
    pub(super) fn request_open_turn_work(&mut self, cx: &mut Context<Self>) {
        if !self.selected_history_current() {
            return;
        }
        let (Some(thread_id), Some(host), Some(state)) = (
            self.selected_thread.clone(),
            self.conversation_host.clone(),
            self.engine_observations.as_ref(),
        ) else {
            return;
        };
        if state.thread_id() != &thread_id {
            return;
        }
        let wanted: Vec<TurnId> = state
            .held_back()
            .map(|held| &held.turn_id)
            .filter(|turn| {
                !self.history_paging.turn_work_in_flight.contains(*turn)
                    && !self.history_paging.turn_work_failed.contains(*turn)
            })
            .filter(|turn| host.read(cx).session_disclosure(turn) != Some(SceneDisclosure::Closed))
            .cloned()
            .collect();
        for turn_id in wanted {
            self.request_turn_work(thread_id.clone(), turn_id, 0);
        }
    }

    fn request_turn_work(&mut self, thread_id: ThreadId, turn_id: TurnId, after_sequence: u64) {
        #[cfg(feature = "flight-recorder")]
        let _trace = artisan_tracing::span!("ui", "request_turn_work");
        let command = NativeTransportCommand::ReadConversationHistory {
            thread_id,
            part: ConversationHistoryPart::TurnWork {
                turn_id: turn_id.clone(),
                after_sequence,
            },
        };
        if self.submit_command(command).is_ok() {
            self.history_paging.turn_work_in_flight.insert(turn_id);
        } else {
            self.history_paging.turn_work_in_flight.remove(&turn_id);
        }
    }

    /// Applies one answered part of the open thread's history.
    pub(super) fn handle_conversation_history(
        &mut self,
        part: ConversationHistoryPart,
        page: ConversationHistoryPage,
        cx: &mut Context<Self>,
    ) {
        if self.selected_thread.as_ref() != Some(&page.thread_id) {
            return;
        }
        match part {
            ConversationHistoryPart::EarlierTurns {
                before_turn_ordinal,
                ..
            } => self.apply_earlier_turns(before_turn_ordinal, page, cx),
            ConversationHistoryPart::TurnWork { turn_id, .. } => {
                self.apply_turn_work(turn_id, &page, cx);
            }
        }
    }

    /// Leaves one unread part of the open thread's history unread.
    pub(super) fn handle_conversation_history_failed(
        &mut self,
        thread_id: &ThreadId,
        part: &ConversationHistoryPart,
    ) {
        if self.selected_thread.as_ref() != Some(thread_id) {
            return;
        }
        match part {
            // The next scroll toward the start asks again.
            ConversationHistoryPart::EarlierTurns { .. } => {
                self.history_paging.earlier_in_flight = false;
                self.history_paging.jump = None;
            }
            ConversationHistoryPart::TurnWork { turn_id, .. } => {
                self.history_paging.turn_work_in_flight.remove(turn_id);
                self.history_paging.turn_work_failed.insert(turn_id.clone());
            }
        }
    }

    fn apply_earlier_turns(
        &mut self,
        before: TurnOrdinal,
        page: ConversationHistoryPage,
        cx: &mut Context<Self>,
    ) {
        self.history_paging.earlier_in_flight = false;
        let (Some(host), Some(earlier)) = (self.conversation_host.clone(), page.snapshot) else {
            return;
        };
        let Some(snapshot) = host.read(cx).canonical_snapshot() else {
            return;
        };
        // The window moved while the page was read (a recovery replaced it):
        // the page no longer joins it.
        if snapshot.turns().first().map(|turn| turn.ordinal) != Some(before) {
            return;
        }
        if earlier.turns().is_empty() {
            self.history_paging.exhausted_before = Some(before.get());
            self.history_paging.jump = None;
            return;
        }
        let items = snapshot.items().len().saturating_add(earlier.items().len());
        let turns = snapshot.turns().len().saturating_add(earlier.turns().len());
        if items > HISTORY_MAX_LOADED_ITEMS || turns > crate::conversation_scene::SCENE_MAX_TURNS {
            self.history_paging.exhausted_before = Some(before.get());
            self.history_paging.jump = None;
            return;
        }
        self.retain_history_observations(&page.observations, &page.held_back);
        let dispatch = host.update(cx, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::EarlierTurnsReceived(
                    earlier,
                )),
                host_cx,
            )
        });
        if dispatch.is_err() {
            self.history_paging.exhausted_before = Some(before.get());
            self.history_paging.jump = None;
            return;
        }
        self.pump_host_boundary(&host, cx);
        self.replay_observation_activity(cx);
        self.sync_earlier_turn_markers(cx);
        self.continue_jump(cx);
        cx.notify();
    }

    fn apply_turn_work(
        &mut self,
        turn_id: TurnId,
        page: &ConversationHistoryPage,
        cx: &mut Context<Self>,
    ) {
        self.retain_history_observations(&page.observations, &[]);
        if let Some(after_sequence) = page.next_after_sequence {
            self.request_turn_work(page.thread_id.clone(), turn_id, after_sequence);
        } else {
            self.history_paging.turn_work_in_flight.remove(&turn_id);
            if let Some(state) = self.engine_observations.as_mut() {
                state.mark_turn_work_loaded(&turn_id);
            }
        }
        self.replay_observation_activity(cx);
        cx.notify();
    }

    /// Pairs rows read on demand into the open thread's observation state.
    fn retain_history_observations(
        &mut self,
        observations: &[artisan_domain::EngineObservationEvent],
        held_back: &[artisan_domain::HeldBackTurnWork],
    ) {
        let Some(state) = self.engine_observations.as_mut() else {
            return;
        };
        for event in observations {
            // Attributed rows pair by their delivery sequence; the wire
            // cursor only orders legacy rows, which history never carries.
            let _ = state.apply(0, event);
        }
        state.add_held_back(held_back);
    }
}

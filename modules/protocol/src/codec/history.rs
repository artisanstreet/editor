//! On-demand conversation history codec: the request for older turns or one
//! turn's held-back work rows, the page that answers it, and the pushed
//! held-back work of a subscription's loaded turns.

use artisan_domain::{
    ConversationHistoryPage, ConversationHistoryPart, ConversationHistoryRequest,
    EarlierTurnMarker, EarlierTurnMarkers, HeldBackTurnWork, HeldBackWork,
};

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn encode_conversation_history_request(
    builder: artisan_capnp::request::Builder<'_>,
    request: &ConversationHistoryRequest,
) {
    let mut encoded = builder.init_conversation_history();
    encoded.set_thread_id(request.thread_id.as_str());
    match &request.part {
        ConversationHistoryPart::EarlierTurns {
            before_turn_ordinal,
            minimum_turn_ordinal,
            maximum_turn_count,
        } => {
            let mut range = encoded.init_part().init_earlier_turns();
            range.set_before_turn_ordinal(before_turn_ordinal.get());
            let mut minimum = range.reborrow().init_minimum_turn_ordinal();
            match minimum_turn_ordinal {
                Some(ordinal) => minimum.set_minimum(ordinal.get()),
                None => minimum.set_no_minimum(()),
            }
            range.set_maximum_turn_count(maximum_turn_count.get());
        }
        ConversationHistoryPart::TurnWork {
            turn_id,
            after_sequence,
        } => {
            let mut range = encoded.init_part().init_turn_work();
            range.set_turn_id(turn_id.as_str());
            range.set_after_sequence(*after_sequence);
        }
    }
}

pub(crate) fn decode_conversation_history_request(
    request: artisan_capnp::conversation_history_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let field = "request.conversationHistory.threadId";
    let thread_id = parse_thread_id(read_text(request.get_thread_id(), field)?, field)?;
    let part = match request.get_part().which()? {
        conversation_history_request::part::Which::EarlierTurns(range) => {
            let range = range?;
            let minimum_turn_ordinal = match range.get_minimum_turn_ordinal().which()? {
                query_range::minimum_turn_ordinal::Which::NoMinimum(()) => None,
                query_range::minimum_turn_ordinal::Which::Minimum(value) => {
                    Some(TurnOrdinal::new(value))
                }
            };
            ConversationHistoryPart::EarlierTurns {
                before_turn_ordinal: TurnOrdinal::new(range.get_before_turn_ordinal()),
                minimum_turn_ordinal,
                maximum_turn_count: QueryTurnCount::new(u64::from(range.get_maximum_turn_count()))?,
            }
        }
        conversation_history_request::part::Which::TurnWork(range) => {
            let range = range?;
            let field = "request.conversationHistory.turnWork.turnId";
            ConversationHistoryPart::TurnWork {
                turn_id: parse_turn_id(read_text(range.get_turn_id(), field)?, field)?,
                after_sequence: range.get_after_sequence(),
            }
        }
    };
    Ok(ClientRequest::Conversation(ConversationRequest::History(
        ConversationHistoryRequest { thread_id, part },
    )))
}

pub(crate) fn encode_conversation_history_page(
    mut builder: artisan_capnp::conversation_history_page::Builder<'_>,
    page: &ConversationHistoryPage,
) -> Result<(), ProtocolEncodeError> {
    builder.set_thread_id(page.thread_id.as_str());
    match &page.snapshot {
        Some(snapshot) => {
            encode_conversation_snapshot(
                builder.reborrow().init_turns().init_snapshot(),
                snapshot,
            )?;
        }
        None => builder.reborrow().init_turns().set_none(()),
    }
    let field = "conversationHistory.observations";
    let mut observations = builder
        .reborrow()
        .init_observations(list_length(field, page.observations.len())?);
    for (index, event) in page.observations.iter().enumerate() {
        encode_engine_observation_event(
            observations.reborrow().get(list_index(field, index)?),
            event,
        )?;
    }
    encode_held_back_turns(
        builder.reborrow().init_held_back(list_length(
            "conversationHistory.heldBack",
            page.held_back.len(),
        )?),
        &page.held_back,
    )?;
    let mut next = builder.init_next();
    match page.next_after_sequence {
        Some(sequence) => next.set_after_sequence(sequence),
        None => next.set_done(()),
    }
    Ok(())
}

pub(crate) fn decode_conversation_history_page(
    page: artisan_capnp::conversation_history_page::Reader<'_>,
) -> Result<ConversationHistoryPage, ProtocolDecodeError> {
    let field = "response.conversationHistory.threadId";
    let thread_id = parse_thread_id(read_text(page.get_thread_id(), field)?, field)?;
    let snapshot = match page.get_turns().which()? {
        conversation_history_page::turns::Which::None(()) => None,
        conversation_history_page::turns::Which::Snapshot(snapshot) => {
            Some(decode_conversation_snapshot(snapshot?)?)
        }
    };
    let observations = page
        .get_observations()?
        .iter()
        .map(decode_engine_observation_event)
        .collect::<Result<Vec<_>, _>>()?;
    let held_back = decode_held_back_turns(page.get_held_back()?)?;
    let next_after_sequence = match page.get_next().which()? {
        conversation_history_page::next::Which::Done(()) => None,
        conversation_history_page::next::Which::AfterSequence(sequence) => Some(sequence),
    };
    Ok(ConversationHistoryPage {
        thread_id,
        snapshot,
        observations,
        held_back,
        next_after_sequence,
    })
}

pub(crate) fn encode_held_back_work(
    mut builder: artisan_capnp::held_back_work::Builder<'_>,
    work: &HeldBackWork,
) -> Result<(), ProtocolEncodeError> {
    builder.set_thread_id(work.thread_id.as_str());
    encode_held_back_turns(
        builder.init_turns(list_length("heldBackWork.turns", work.turns.len())?),
        &work.turns,
    )
}

pub(crate) fn decode_held_back_work(
    work: artisan_capnp::held_back_work::Reader<'_>,
) -> Result<HeldBackWork, ProtocolDecodeError> {
    let field = "event.heldBackWork.threadId";
    Ok(HeldBackWork {
        thread_id: parse_thread_id(read_text(work.get_thread_id(), field)?, field)?,
        turns: decode_held_back_turns(work.get_turns()?)?,
    })
}

fn encode_held_back_turns(
    mut builder: capnp::struct_list::Builder<'_, artisan_capnp::held_back_turn_work::Owned>,
    turns: &[HeldBackTurnWork],
) -> Result<(), ProtocolEncodeError> {
    for (index, turn) in turns.iter().enumerate() {
        let mut encoded = builder
            .reborrow()
            .get(list_index("heldBackWork.turns", index)?);
        encoded.set_turn_id(turn.turn_id.as_str());
        encoded.set_run_id(turn.run_id.as_str());
        encoded.set_row_count(turn.row_count);
        encoded.set_first_committed_at_millis(turn.first_committed_at.as_millis());
        encoded.set_first_delivery_sequence(turn.first_delivery_sequence);
    }
    Ok(())
}

fn decode_held_back_turns(
    turns: capnp::struct_list::Reader<'_, artisan_capnp::held_back_turn_work::Owned>,
) -> Result<Vec<HeldBackTurnWork>, ProtocolDecodeError> {
    turns
        .iter()
        .map(|turn| {
            let turn_field = "heldBackWork.turns.turnId";
            let run_field = "heldBackWork.turns.runId";
            Ok(HeldBackTurnWork {
                turn_id: parse_turn_id(read_text(turn.get_turn_id(), turn_field)?, turn_field)?,
                run_id: parse_run_id(read_text(turn.get_run_id(), run_field)?, run_field)?,
                row_count: turn.get_row_count(),
                first_committed_at: UnixMillis::from_millis(turn.get_first_committed_at_millis()),
                first_delivery_sequence: turn.get_first_delivery_sequence(),
            })
        })
        .collect()
}

pub(crate) fn encode_earlier_turn_markers(
    mut builder: artisan_capnp::earlier_turn_markers::Builder<'_>,
    value: &EarlierTurnMarkers,
) -> Result<(), ProtocolEncodeError> {
    builder.set_thread_id(value.thread_id.as_str());
    let field = "earlierTurnMarkers.markers";
    let mut markers = builder.init_markers(list_length(field, value.markers.len())?);
    for (index, marker) in value.markers.iter().enumerate() {
        let mut encoded = markers.reborrow().get(list_index(field, index)?);
        encoded.set_item_id(marker.item_id.as_str());
        encoded.set_turn_ordinal(marker.turn_ordinal.get());
        encoded.set_label(marker.label.as_str());
    }
    Ok(())
}

pub(crate) fn decode_earlier_turn_markers(
    value: artisan_capnp::earlier_turn_markers::Reader<'_>,
) -> Result<EarlierTurnMarkers, ProtocolDecodeError> {
    let field = "event.earlierTurnMarkers.threadId";
    let thread_id = parse_thread_id(read_text(value.get_thread_id(), field)?, field)?;
    let markers = value
        .get_markers()?
        .iter()
        .map(|marker| {
            let field = "event.earlierTurnMarkers.markers.itemId";
            Ok(EarlierTurnMarker {
                item_id: parse_item_id(read_text(marker.get_item_id(), field)?, field)?,
                turn_ordinal: TurnOrdinal::new(marker.get_turn_ordinal()),
                label: read_text(marker.get_label(), "event.earlierTurnMarkers.markers.label")?,
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;
    Ok(EarlierTurnMarkers { thread_id, markers })
}

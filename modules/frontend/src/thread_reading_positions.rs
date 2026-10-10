//! Where the reader left each thread they switched away from.
//!
//! A position is only as good as the transcript it was taken in. Each one is
//! stamped with the thread's patch cursor when the reader left; reopening the
//! thread restores it only when the delivered snapshot is at that same
//! cursor. Anything that moved the thread since (a reply, an edit, a removed
//! turn) advanced the cursor, so the position is dropped and the thread opens
//! at its end as usual. A position is used once: leaving again records a new
//! one.

use artisan_domain::{ConversationCursor, ThreadId};

use crate::conversation_surface::TranscriptReadingPosition;

/// How many threads keep a position; the least recently left goes first.
pub const THREAD_READING_POSITIONS_MAX: usize = 32;

/// Reading positions by thread, bounded and oldest-first.
#[derive(Debug, Default)]
pub struct ThreadReadingPositions {
    entries: Vec<(ThreadId, ConversationCursor, TranscriptReadingPosition)>,
}

impl ThreadReadingPositions {
    /// Records where the reader left `thread`, or forgets it when they left
    /// at the end (`None`).
    pub fn remember(
        &mut self,
        thread: &ThreadId,
        cursor: ConversationCursor,
        position: Option<TranscriptReadingPosition>,
    ) {
        self.entries.retain(|(entry, _, _)| entry != thread);
        let Some(position) = position else {
            return;
        };
        if self.entries.len() >= THREAD_READING_POSITIONS_MAX {
            self.entries.remove(0);
        }
        self.entries.push((thread.clone(), cursor, position));
    }

    /// Takes the position for `thread` if the thread is still at the cursor
    /// it was left at. Either way the thread keeps no position afterwards.
    pub fn take_current(
        &mut self,
        thread: &ThreadId,
        cursor: ConversationCursor,
    ) -> Option<TranscriptReadingPosition> {
        let index = self
            .entries
            .iter()
            .position(|(entry, _, _)| entry == thread)?;
        let (_, left_at, position) = self.entries.remove(index);
        (left_at == cursor).then_some(position)
    }
}

#[cfg(test)]
mod tests {
    use artisan_domain::TurnId;

    use super::*;

    fn thread(value: &str) -> ThreadId {
        ThreadId::parse(value).expect("thread id is valid")
    }

    fn position(turn: &str) -> TranscriptReadingPosition {
        TranscriptReadingPosition::at_turn(TurnId::parse(turn).expect("turn id is valid"))
    }

    fn cursor(value: u64) -> ConversationCursor {
        ConversationCursor::new(value)
    }

    #[test]
    fn an_unmoved_thread_gets_its_position_back_once() {
        let mut positions = ThreadReadingPositions::default();
        positions.remember(&thread("thread_a"), cursor(7), Some(position("turn_a")));

        assert_eq!(
            positions.take_current(&thread("thread_a"), cursor(7)),
            Some(position("turn_a"))
        );
        assert_eq!(
            positions.take_current(&thread("thread_a"), cursor(7)),
            None,
            "a position is used once; leaving again records a new one"
        );
    }

    #[test]
    fn a_thread_that_moved_since_drops_its_position() {
        let mut positions = ThreadReadingPositions::default();
        positions.remember(&thread("thread_a"), cursor(7), Some(position("turn_a")));

        assert_eq!(positions.take_current(&thread("thread_a"), cursor(8)), None);
        assert_eq!(
            positions.take_current(&thread("thread_a"), cursor(7)),
            None,
            "the stale position is cleared, not kept for a later match"
        );
    }

    #[test]
    fn leaving_at_the_end_forgets_an_earlier_position() {
        let mut positions = ThreadReadingPositions::default();
        positions.remember(&thread("thread_a"), cursor(7), Some(position("turn_a")));
        positions.remember(&thread("thread_a"), cursor(7), None);

        assert_eq!(positions.take_current(&thread("thread_a"), cursor(7)), None);
    }

    #[test]
    fn the_least_recently_left_thread_goes_first() {
        let mut positions = ThreadReadingPositions::default();
        for index in 0..=THREAD_READING_POSITIONS_MAX {
            positions.remember(
                &thread(&format!("thread_{index:02}")),
                cursor(1),
                Some(position("turn_a")),
            );
        }

        assert_eq!(
            positions.take_current(&thread("thread_00"), cursor(1)),
            None
        );
        assert!(
            positions
                .take_current(&thread("thread_01"), cursor(1))
                .is_some()
        );
        let newest = format!("thread_{THREAD_READING_POSITIONS_MAX:02}");
        assert!(
            positions
                .take_current(&thread(&newest), cursor(1))
                .is_some()
        );
    }
}

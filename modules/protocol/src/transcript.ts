import { Schema } from "effect";

import { Identifier, IsoDateTime, JournalSequence, PositiveInt } from "./common";
import { ProjectRef } from "./thread";

/** A bounded, renderer-safe fact derived from the durable journal. */
export const TranscriptEntry = Schema.Struct({
	event_id: Identifier,
	journal_sequence: JournalSequence,
	occurred_at: IsoDateTime,
	payload: Schema.Union([
		Schema.Struct({
			type: Schema.Literal("thread.message_queued"),
			message_id: Identifier,
			text: Schema.NonEmptyString,
			working_directory: Schema.NonEmptyString,
		}),
		Schema.Struct({
			type: Schema.Literal("thread.message_steering"),
			message_id: Identifier,
			text: Schema.NonEmptyString,
			working_directory: Schema.NonEmptyString,
			mentioned_projects: Schema.optional(Schema.Array(ProjectRef)),
		}),
		Schema.Struct({
			type: Schema.Literal("assistant.message_completed"),
			message_id: Identifier,
			text: Schema.NonEmptyString,
		}),
		Schema.Struct({
			type: Schema.Literal("interaction.approval"),
			approval_id: Identifier,
			approved: Schema.optional(Schema.Boolean),
			description: Schema.NonEmptyString,
			state: Schema.Literals(["requested", "resolved"]),
		}),
		Schema.Struct({
			type: Schema.Literal("interaction.question"),
			question_id: Identifier,
			text: Schema.NonEmptyString,
			state: Schema.Literals(["requested", "resolved"]),
			source: Schema.optional(Schema.Literals(["engine", "intake"])),
		}),
		Schema.Struct({
			type: Schema.Literal("intake.assessed"),
			message_id: Identifier,
			risk: Schema.Literals(["low", "material", "high", "underspecified"]),
			resolution: Schema.Literals(["proceed", "question"]),
		}),
		Schema.Struct({
			type: Schema.Literal("intake.assumption_recorded"),
			message_id: Identifier,
			assumption: Schema.NonEmptyString,
		}),
	]),
});

export type TranscriptEntry = typeof TranscriptEntry.Type;

export const ThreadTranscriptQuery = Schema.Struct({
	after_journal_sequence: Schema.optional(JournalSequence),
	before_journal_sequence: Schema.optional(JournalSequence),
	limit: Schema.optional(PositiveInt),
	thread_id: Identifier,
});
export type ThreadTranscriptQuery = typeof ThreadTranscriptQuery.Type;

/** Explicitly distinguishes an erased thread from one that has never existed. */
export const ThreadTranscriptSnapshot = Schema.Union([
	Schema.Struct({
		status: Schema.Literal("available"),
		journal_sequence: JournalSequence,
		next_before_journal_sequence: Schema.optional(JournalSequence),
		entries: Schema.Array(TranscriptEntry),
	}),
	Schema.Struct({
		status: Schema.Literal("erased"),
		journal_sequence: JournalSequence,
		next_before_journal_sequence: Schema.optional(JournalSequence),
		entries: Schema.Array(TranscriptEntry),
	}),
	Schema.Struct({
		status: Schema.Literal("unavailable"),
		journal_sequence: JournalSequence,
		next_before_journal_sequence: Schema.optional(JournalSequence),
		entries: Schema.Array(TranscriptEntry),
	}),
]);
export type ThreadTranscriptSnapshot = typeof ThreadTranscriptSnapshot.Type;

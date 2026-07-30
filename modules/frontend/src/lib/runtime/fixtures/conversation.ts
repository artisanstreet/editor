import { Schema } from "effect";

import { ConversationSnapshot as ConversationSnapshotSchema } from "@artisan/protocol";
import type { ConversationSnapshot } from "@artisan/protocol";

export const FixtureConversation = (thread_id: string): ConversationSnapshot => {
	const created_at = "2026-07-24T00:00:00.000Z";
	const Entity = (
		id: string,
		ordinal: number,
		lifecycle: "active" | "completed" = "completed",
	) => ({
		created_at,
		id,
		lifecycle,
		ordinal,
		references: [],
		revision: 1,
		source_refs: [{ provider: "fixture", reference: `fixture:${id}` }],
		updated_at: created_at,
	});
	const turn_ids = ["turn-1", "turn-2", "turn-3", "turn-4"] as const;
	const turn_ordinals = [0, 6, 14, 19] as const;

	return Schema.decodeUnknownSync(ConversationSnapshotSchema)({
		conversation_id: `conversation:${thread_id}`,
		items: [
			{
				...Entity("message-user-1", 1),
				text: "Can you make the thread transcript stable while tools, reasoning, and file changes stream in?",
				turn_id: turn_ids[0],
				type: "user_message",
			},
			{
				...Entity("reasoning-1", 2),
				text: "I’ll first map the existing event flow, then introduce one deterministic conversation projection rather than asking each component to infer state.",
				turn_id: turn_ids[0],
				type: "reasoning_summary",
			},
			{
				...Entity("work-1", 3),
				ended_at: created_at,
				started_at: created_at,
				status: "completed",
				title: "Mapped the engine and transcript pipeline",
				turn_id: turn_ids[0],
				type: "work_session",
			},
			{
				...Entity("activity-1", 4),
				detail: "Read the normalizer, journal, transport, and renderer boundaries.",
				kind: "read",
				label: "Inspected conversation flow",
				status: "completed",
				turn_id: turn_ids[0],
				type: "activity",
			},
			{
				...Entity("message-assistant-1", 5),
				phase: "final",
				text: "The brittle part was not rendering itself. Several layers were independently guessing how provider events belonged together. I’ve reduced that to one typed stream with stable identities.",
				turn_id: turn_ids[0],
				type: "assistant_message",
			},
			{
				...Entity("message-user-2", 7),
				text: "Good. Make changed files and work sessions first-class instead of parsing headings.",
				turn_id: turn_ids[1],
				type: "user_message",
			},
			{
				...Entity("work-2", 8),
				ended_at: created_at,
				started_at: created_at,
				status: "completed",
				title: "Built the canonical conversation reducer",
				turn_id: turn_ids[1],
				type: "work_session",
			},
			{
				...Entity("activity-2", 9),
				detail: "Added ordered turns, messages, work sessions, activities, and changes.",
				kind: "write",
				label: "Defined renderer-ready entities",
				status: "completed",
				turn_id: turn_ids[1],
				type: "activity",
			},
			{
				...Entity("change-set-1", 10),
				file_count: 3,
				file_ids: ["file-protocol", "file-projection", "file-renderer"],
				state: "applied",
				summary: "Added the conversation protocol, durable projection, and typed renderer",
				turn_id: turn_ids[1],
				type: "change_set",
			},
			{
				...Entity("file-change-1", 11),
				change_set_id: "change-set-1",
				diff: { additions: 127, deletions: 8, kind: "known" },
				operation: "created",
				path: "modules/protocol/src/conversation.ts",
				turn_id: turn_ids[1],
				type: "file_change",
			},
			{
				...Entity("file-change-2", 12),
				change_set_id: "change-set-1",
				diff: { additions: 42, deletions: 3, kind: "known" },
				operation: "created",
				path: "modules/backend/src/conversation/projection-api.ts",
				turn_id: turn_ids[1],
				type: "file_change",
			},
			{
				...Entity("message-assistant-2", 13),
				phase: "final",
				text: "Changed files now arrive as explicit change-set and file-change entities. “Worked for” is a work-session lifecycle, so neither relies on timing or text heuristics.",
				turn_id: turn_ids[1],
				type: "assistant_message",
			},
			{
				...Entity("message-user-3", 15),
				text: "What happens if the stream reconnects or sends the same patch twice?",
				turn_id: turn_ids[2],
				type: "user_message",
			},
			{
				...Entity("reasoning-2", 16),
				text: "The frontend should apply only contiguous revisions. A gap, identity mismatch, or illegal lifecycle transition must trigger a snapshot resync instead of producing a half-valid UI.",
				turn_id: turn_ids[2],
				type: "reasoning_summary",
			},
			{
				...Entity("activity-3", 17),
				detail: "Replayed duplicate, delayed, and out-of-order patches against the reducer.",
				kind: "test",
				label: "Exercised recovery behavior",
				status: "completed",
				turn_id: turn_ids[2],
				type: "activity",
			},
			{
				...Entity("message-assistant-3", 18),
				phase: "final",
				text: "Duplicate patch IDs are idempotent. Sequence gaps and invalid transitions request a clean snapshot, while completed entities remain immutable. The transcript no longer flickers between interpretations.",
				turn_id: turn_ids[2],
				type: "assistant_message",
			},
			{
				...Entity("message-user-4", 20),
				text: "Make sure the mock is long enough to judge scrolling and the sticky composer.",
				turn_id: turn_ids[3],
				type: "user_message",
			},
			{
				...Entity("work-3", 21, "active"),
				started_at: created_at,
				status: "active",
				title: "Rendering the deterministic thread fixture",
				turn_id: turn_ids[3],
				type: "work_session",
			},
			{
				...Entity("activity-4", 22, "active"),
				detail: "Populating the mock through the same protocol schema used by live threads.",
				kind: "render",
				label: "Prepared visual fixture",
				status: "active",
				turn_id: turn_ids[3],
				type: "activity",
			},
			{
				...Entity("message-assistant-4", 23),
				phase: "commentary",
				text: "The mock now covers enough distinct entity types and vertical space to inspect transcript scrolling, grouping, status treatments, and the sticky glass composer without inventing a second UI-only data shape.",
				turn_id: turn_ids[3],
				type: "assistant_message",
			},
		],
		journal_sequence: 48,
		last_patch_sequence: 0,
		schema_version: 1,
		thread_id,
		turns: turn_ids.map((id, index) => ({
			...Entity(
				id,
				turn_ordinals[index] ?? index,
				index === turn_ids.length - 1 ? "active" : "completed",
			),
			type: "turn",
		})),
		updated_at: created_at,
	});
};

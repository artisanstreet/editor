import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	ApplyConversationPatch,
	ConversationItem,
	ConversationPatch,
	ConversationSnapshot,
	conversation_body_text_limit,
	InitializeConversation,
	RebuildConversation,
} from "@artisan/protocol";

const at = "2026-07-24T10:00:00.000Z";

const turn = {
	created_at: at,
	id: "turn_1",
	lifecycle: "streaming" as const,
	ordinal: 1,
	references: [],
	revision: 0,
	source_refs: [{ reference: "event_1" }],
	type: "turn" as const,
	updated_at: at,
};

const assistant = {
	created_at: at,
	id: "item_1",
	lifecycle: "streaming" as const,
	ordinal: 0,
	references: [],
	revision: 0,
	source_refs: [{ reference: "event_2" }],
	phase: "unspecified" as const,
	text: "",
	turn_id: "turn_1",
	type: "assistant_message" as const,
	updated_at: at,
};

const snapshot = {
	conversation_id: "conversation_1",
	items: [],
	journal_sequence: 0,
	last_patch_sequence: 0,
	schema_version: 1 as const,
	thread_id: "thread_1",
	turns: [turn],
	updated_at: at,
};

const decode_patch = Schema.decodeUnknownSync(ConversationPatch, { onExcessProperty: "error" });

describe("canonical conversation protocol", () => {
	it("validates every normalized item discriminator and rejects UI-shaped aliases", () => {
		const decode_item = Schema.decodeUnknownSync(ConversationItem, {
			onExcessProperty: "error",
		});
		const { phase: _phase, text: _text, ...item_base } = assistant;
		for (const type of [
			"user_message",
			"assistant_message",
			"reasoning_summary",
			"work_session",
			"activity",
			"change_set",
			"file_change",
			"plan",
			"approval",
			"question",
			"error",
			"compaction",
			"native_event",
			"model_transition",
		]) {
			const payload =
				type === "work_session"
					? { ended_at: at, started_at: at, status: "completed", title: "Work" }
					: type === "activity"
						? { detail: "Running", kind: "tool", label: "Running", status: "active" }
						: type === "file_change"
							? {
									change_set_id: "change_1",
									diff: { additions: 4, deletions: 2, kind: "known" },
									operation: "modified",
									path: "src/file.ts",
								}
							: type === "change_set"
								? {
										file_count: 0,
										file_ids: [],
										state: "pending",
										summary: "Summary",
									}
								: type === "compaction"
									? {
											portability: "portable",
											state: "completed",
											summary: "Summary",
										}
									: type === "model_transition"
										? {
												continuation: "portable",
												source_engine_id: "claude",
												source_model_id: "claude-sonnet",
												state: "completed",
												target_engine_id: "codex",
												target_model_id: "gpt-5",
											}
										: type === "native_event"
											? { summary: "Summary" }
											: type === "plan"
												? { entries: [], state: "draft" }
												: ["approval", "question"].includes(type)
													? {
															interaction_id: `interaction_${type}`,
															prompt: "Continue?",
															requested_at: at,
															state: "requested",
														}
													: type === "error"
														? {
																message: "Failed",
																retry: { kind: "none" },
															}
														: {
																text:
																	type === "assistant_message" ||
																	type === "reasoning_summary"
																		? ""
																		: "Text",
																...(type === "assistant_message"
																	? { phase: "final" }
																	: {}),
															};
			expect(
				decode_item({ ...item_base, ...payload, id: `item_${type}`, type }),
			).toMatchObject({ type });
		}
		expect(() => decode_item({ ...assistant, type: "header" })).toThrow();
	});

	/**
	 * A label cut at 4096 characters is still a label. A reply cut there ends
	 * mid-sentence and the turn reads as answered, so bodies carry their own
	 * bound while every label-sized field keeps the narrow one.
	 */
	it("lets a message body outgrow the label bound and still bounds it", () => {
		const decode_item = Schema.decodeUnknownSync(ConversationItem);
		const long_reply = "word ".repeat(2_000);

		expect(long_reply.length).toBeGreaterThan(4_096);
		expect(long_reply.length).toBeLessThanOrEqual(conversation_body_text_limit);
		expect(decode_item({ ...assistant, text: long_reply })).toMatchObject({
			text: long_reply,
		});
		expect(
			decode_item({ ...assistant, text: "x".repeat(conversation_body_text_limit) }),
		).toMatchObject({ lifecycle: "streaming" });
		expect(() =>
			decode_item({ ...assistant, text: "x".repeat(conversation_body_text_limit + 1) }),
		).toThrow();
		expect(() =>
			decode_item({
				...assistant,
				kind: "tool_activity",
				label: "x".repeat(4_097),
				status: "completed",
				text: undefined,
				type: "activity",
			}),
		).toThrow();
	});

	it("decodes legacy assistant messages without a disclosed phase as unspecified", () => {
		const { phase: _phase, ...legacy_assistant } = assistant;
		expect(Schema.decodeUnknownSync(ConversationItem)(legacy_assistant)).toMatchObject({
			phase: "unspecified",
			type: "assistant_message",
		});
	});

	it("decodes typed approval requests without invalidating legacy approval rows", () => {
		const decode_item = Schema.decodeUnknownSync(ConversationItem, {
			onExcessProperty: "error",
		});
		const { phase: _phase, text: _text, ...item_base } = assistant;
		const approval_base = {
			...item_base,
			id: "approval_1",
			interaction_id: "opaque_response_id",
			lifecycle: "waiting",
			prompt: "Run the test suite",
			requested_at: at,
			state: "requested",
			type: "approval",
		};

		expect(
			decode_item({
				...approval_base,
				request: {
					command: "pnpm test",
					cwd: "C:\\workspace",
					kind: "command",
					reason: "Run the test suite",
				},
			}),
		).toMatchObject({
			interaction_id: "opaque_response_id",
			request: {
				command: "pnpm test",
				cwd: "C:\\workspace",
				kind: "command",
			},
		});
		expect(decode_item(approval_base)).toMatchObject({
			interaction_id: "opaque_response_id",
			type: "approval",
		});
	});

	it("rebuilds streaming text in order and makes exact replay idempotent", () => {
		const patches = [
			decode_patch({
				patch_id: "patch_1",
				sequence: 1,
				type: "item_upsert",
				item: assistant,
			}),
			decode_patch({
				patch_id: "patch_2",
				sequence: 2,
				type: "item_append",
				item_id: "item_1",
				revision: 1,
				text: "Hello",
			}),
			decode_patch({
				patch_id: "patch_3",
				sequence: 3,
				type: "item_lifecycle",
				item_id: "item_1",
				lifecycle: "completed",
				revision: 2,
			}),
		];
		const result = RebuildConversation(snapshot, patches);
		expect(result).toMatchObject({
			_tag: "applied",
			state: {
				snapshot: {
					last_patch_sequence: 3,
					items: [{ text: "Hello", lifecycle: "completed" }],
				},
			},
		});
		if (result._tag !== "applied") throw new Error("Expected an applied rebuild");
		const completed_patch = patches[2];
		if (completed_patch === undefined) throw new Error("Expected a completed patch");
		expect(ApplyConversationPatch(result.state, completed_patch)).toMatchObject({
			_tag: "duplicate",
		});
	});

	it("rejects gaps, mutable terminal entities, changed ordinals, and broken references", () => {
		const initial = InitializeConversation(snapshot);
		if (initial._tag !== "applied") throw new Error("Expected a valid snapshot");
		expect(
			ApplyConversationPatch(
				initial.state,
				decode_patch({
					patch_id: "gap",
					sequence: 2,
					type: "item_upsert",
					item: assistant,
				}),
			),
		).toMatchObject({ _tag: "invariant_error", error: { code: "patch_gap" } });
		const terminal = RebuildConversation(snapshot, [
			decode_patch({
				patch_id: "create",
				sequence: 1,
				type: "item_upsert",
				item: { ...assistant, lifecycle: "completed" },
			}),
			decode_patch({
				patch_id: "mutate",
				sequence: 2,
				type: "item_upsert",
				item: { ...assistant, lifecycle: "completed", revision: 1, text: "No" },
			}),
		]);
		expect(terminal).toMatchObject({
			_tag: "invariant_error",
			error: { code: "terminal_immutable" },
		});
		const invalid = Schema.decodeUnknownSync(ConversationSnapshot)({
			...snapshot,
			items: [{ ...assistant, parent_id: "missing" }],
		});
		expect(InitializeConversation(invalid)).toMatchObject({
			_tag: "invariant_error",
			error: { code: "invalid_parent" },
		});
		const duplicate_ordinal = Schema.decodeUnknownSync(ConversationSnapshot)({
			...snapshot,
			items: [{ ...assistant, ordinal: turn.ordinal }],
		});
		expect(InitializeConversation(duplicate_ordinal)).toMatchObject({
			_tag: "invariant_error",
			error: { code: "duplicate_ordinal" },
		});
		const replace_with_invalid_reference = RebuildConversation(snapshot, [
			decode_patch({
				item: assistant,
				patch_id: "create-referenced",
				sequence: 1,
				type: "item_upsert",
			}),
			decode_patch({
				item: {
					...assistant,
					references: ["missing"],
					revision: 1,
				},
				patch_id: "replace-invalid-reference",
				sequence: 2,
				type: "item_upsert",
			}),
		]);
		expect(replace_with_invalid_reference).toMatchObject({
			_tag: "invariant_error",
			error: { code: "invalid_reference" },
		});
	});

	it("allows the explicit active and waiting lifecycle graph before a terminal transition", () => {
		const result = RebuildConversation(snapshot, [
			decode_patch({
				patch_id: "active",
				sequence: 1,
				type: "turn_lifecycle",
				turn_id: "turn_1",
				lifecycle: "active",
				revision: 1,
			}),
			decode_patch({
				patch_id: "waiting",
				sequence: 2,
				type: "turn_lifecycle",
				turn_id: "turn_1",
				lifecycle: "waiting",
				revision: 2,
			}),
			decode_patch({
				patch_id: "resumed",
				sequence: 3,
				type: "turn_lifecycle",
				turn_id: "turn_1",
				lifecycle: "active",
				revision: 3,
			}),
			decode_patch({
				patch_id: "done",
				sequence: 4,
				type: "turn_lifecycle",
				turn_id: "turn_1",
				lifecycle: "completed",
				revision: 4,
			}),
		]);
		expect(result).toMatchObject({
			_tag: "applied",
			state: { snapshot: { turns: [{ lifecycle: "completed", revision: 4 }] } },
		});
	});
});

import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import { ConversationItem, ConversationSnapshot } from "@artisan/protocol";

const decode = (schema: typeof ConversationItem | typeof ConversationSnapshot, value: unknown) =>
	Effect.runSync(
		Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(value).pipe(
			Effect.map((decoded) => ({ decoded, ok: true as const })),
			Effect.catch((issue) => Effect.succeed({ issue: String(issue), ok: false as const })),
		),
	);

/** A handoff row exactly as builds before `state` existed wrote it. */
const legacy_model_transition = {
	continuation: "portable",
	created_at: "2026-07-30T14:56:44.985Z",
	id: "model-transition:event_1",
	lifecycle: "completed",
	ordinal: 23,
	references: [],
	revision: 0,
	run_id: "run_1",
	source_engine_id: "codex",
	source_model_id: "gpt-5.6-sol",
	source_refs: [{ event_id: "event_1", journal_sequence: 63, reference: "event_1" }],
	target_engine_id: "claude",
	target_model_id: "claude-opus-5",
	turn_id: "run:run_1",
	type: "model_transition",
	updated_at: "2026-07-30T14:56:44.985Z",
};

describe("stored conversation items written before a field existed", () => {
	/**
	 * The projection decodes every stored item as one unit, so a single
	 * undecodable row fails the whole snapshot read for that thread — the
	 * transcript goes blank rather than losing one handoff line. A new required
	 * field on an existing item type must therefore always carry a decoding
	 * default.
	 */
	it("decodes a model transition that predates the state field", () => {
		const result = decode(ConversationItem, legacy_model_transition);

		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.decoded).toMatchObject({ state: "completed", type: "model_transition" });
	});

	it("keeps an explicit in-flight state rather than defaulting over it", () => {
		const result = decode(ConversationItem, { ...legacy_model_transition, state: "started" });

		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.decoded).toMatchObject({ state: "started" });
	});

	it("reads a whole snapshot containing a legacy handoff", () => {
		const result = decode(ConversationSnapshot, {
			conversation_id: "conversation:thread_1",
			items: [legacy_model_transition],
			journal_sequence: 63,
			last_patch_sequence: 56,
			schema_version: 1,
			thread_id: "thread_1",
			turns: [
				{
					created_at: "2026-07-30T14:56:44.985Z",
					id: "run:run_1",
					lifecycle: "completed",
					ordinal: 22,
					references: [],
					revision: 0,
					run_id: "run_1",
					source_refs: [{ reference: "event_1" }],
					type: "turn",
					updated_at: "2026-07-30T14:56:44.985Z",
				},
			],
			updated_at: "2026-07-30T14:56:44.985Z",
		});

		expect(result.ok).toBe(true);
	});
});

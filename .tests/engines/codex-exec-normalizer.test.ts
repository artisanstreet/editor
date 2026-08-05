import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { NormaliseCodexExecEvent } from "../../modules/engines/src/codex/exec-normalizer";

describe("Codex exec normalizer usage", () => {
	it("keeps context-compaction start and completion on one native lifecycle identity", async () => {
		const normalize = (frame_sequence: number, type: "item.started" | "item.completed") =>
			Effect.runPromise(
				NormaliseCodexExecEvent({
					artisan_run_id: "run_1",
					frame_sequence,
					payload: {
						item: { id: "compact-1", type: "context_compaction" },
						type,
					},
					raw_frame_base64: "e30=",
					turn_id: "turn_1",
				}),
			);

		const [[started], [completed]] = await Promise.all([
			normalize(1, "item.started"),
			normalize(2, "item.completed"),
		]);

		expect(started).toMatchObject({
			_tag: "compaction",
			compaction_id: "compact-1",
			state: "started",
		});
		expect(completed).toMatchObject({
			_tag: "compaction",
			compaction_id: "compact-1",
			state: "completed",
		});
	});

	it("keeps completed agent messages unspecified when exec supplies no phase", async () => {
		const observations = await Effect.runPromise(
			NormaliseCodexExecEvent({
				artisan_run_id: "run_1",
				frame_sequence: 1,
				payload: {
					item: { id: "assistant-item-1", text: "Done", type: "agent_message" },
					type: "item.completed",
				},
				raw_frame_base64: "e30=",
				turn_id: "turn_1",
			}),
		);

		expect(observations).toContainEqual(
			expect.objectContaining({
				_tag: "agent_message_completed",
				item_id: "run_1:exec:item:assistant-item-1",
				phase: "unspecified",
			}),
		);
	});

	it("keeps repeated native item identities isolated between runs", async () => {
		const normalize = (artisan_run_id: string) =>
			Effect.runPromise(
				NormaliseCodexExecEvent({
					artisan_run_id,
					frame_sequence: 1,
					payload: {
						item: { id: "item_0", text: "Done", type: "agent_message" },
						type: "item.completed",
					},
					raw_frame_base64: "e30=",
					turn_id: `turn:${artisan_run_id}`,
				}),
			);
		const [first, second] = await Promise.all([normalize("run_1"), normalize("run_2")]);

		expect(first).toContainEqual(
			expect.objectContaining({ item_id: "run_1:exec:item:item_0" }),
		);
		expect(second).toContainEqual(
			expect.objectContaining({ item_id: "run_2:exec:item:item_0" }),
		);
	});

	it("marks turn-completed token usage as a delta", async () => {
		const observations = await Effect.runPromise(
			NormaliseCodexExecEvent({
				artisan_run_id: "run_1",
				frame_sequence: 1,
				payload: {
					type: "turn.completed",
					usage: { input_tokens: 12, output_tokens: 4 },
				},
				raw_frame_base64: "e30=",
				turn_id: "turn_1",
			}),
		);

		expect(observations).toContainEqual(
			expect.objectContaining({
				_tag: "usage",
				basis: "delta",
				input_tokens: 12,
				output_tokens: 4,
			}),
		);
	});

	it("closes the reasoning phase on a completed reasoning item that never streamed a delta", async () => {
		const observations = await Effect.runPromise(
			NormaliseCodexExecEvent({
				artisan_run_id: "run_1",
				frame_sequence: 1,
				payload: {
					item: {
						id: "reasoning-1",
						text: "Inspecting the adapter contract.",
						type: "reasoning",
					},
					type: "item.completed",
				},
				raw_frame_base64: "e30=",
				turn_id: "turn_1",
			}),
		);

		expect(observations).toEqual([
			expect.objectContaining({
				_tag: "reasoning_summary_completed",
				item_id: "run_1:exec:item:reasoning-1",
				turn_id: "turn_1",
			}),
		]);
	});

	it("keeps a started or updated reasoning item as a native action, not a completion", async () => {
		const observations = await Effect.runPromise(
			NormaliseCodexExecEvent({
				artisan_run_id: "run_1",
				frame_sequence: 1,
				payload: {
					item: { id: "reasoning-1", type: "reasoning" },
					type: "item.started",
				},
				raw_frame_base64: "e30=",
				turn_id: "turn_1",
			}),
		);

		/** Where the text was kept describes the adapter; the row says what happened. */
		expect(observations).toEqual([
			expect.objectContaining({ _tag: "native_action", detail: "Reasoning started" }),
		]);
	});

	it("rejects invalid token counts instead of normalizing usage", async () => {
		const observations = await Effect.runPromise(
			NormaliseCodexExecEvent({
				artisan_run_id: "run_1",
				frame_sequence: 2,
				payload: {
					type: "turn.completed",
					usage: { input_tokens: -1, output_tokens: 1.5 },
				},
				raw_frame_base64: "e30=",
				turn_id: "turn_1",
			}),
		);

		expect(observations).not.toContainEqual(expect.objectContaining({ _tag: "usage" }));
	});
});

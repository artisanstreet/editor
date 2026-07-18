import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { NormaliseCodexExecEvent } from "../../modules/engines/src/codex/codex-exec-normalizer";

describe("Codex exec normalizer usage", () => {
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

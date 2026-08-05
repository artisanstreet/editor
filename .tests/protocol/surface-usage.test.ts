import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import { SurfaceUsageAggregate } from "@artisan/protocol";

describe("surface usage aggregate", () => {
	it("decodes immutable context-gauge run provenance independently of policy", () => {
		const aggregate = Schema.decodeUnknownSync(SurfaceUsageAggregate)({
			context_origin: {
				engine_id: "codex",
				model_id: "gpt-5.6-luna",
				run_id: "run_luna",
			},
			context_tokens: 128_000,
			context_window_tokens: 258_400,
			scope: "run",
			scope_id: "run_luna",
		});

		expect(aggregate.context_origin).toEqual({
			engine_id: "codex",
			model_id: "gpt-5.6-luna",
			run_id: "run_luna",
		});
	});
});

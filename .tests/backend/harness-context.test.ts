import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import type { EngineDescriptor } from "@artisan/engines";
import {
	ArtisanHarnessContext,
	ArtisanHarnessContextLive,
	artisan_harness_context_version,
} from "../../modules/backend/src/harness/harness-context";

function descriptor(
	harness_context: "experimental" | "supported" | "unsupported",
): EngineDescriptor {
	return {
		capabilities: {
			approval: { state: "supported" },
			auth: { state: "supported" },
			cancel: { state: "supported" },
			close: { state: "supported" },
			events: { state: "supported" },
			global_guidance: { state: "supported" },
			harness_context: { state: harness_context },
			model_selection: { state: "supported" },
			native_tools: { state: "supported" },
			probe: { state: "supported" },
			question: { state: "supported" },
			raw_frames: { state: "supported" },
			resume: { state: "supported" },
			start: { state: "supported" },
			steer: { state: "supported" },
			subagents: { state: "supported" },
		},
		display_name: "Fixture engine",
		id: "fixture",
		transport: "fixture",
	};
}

describe("Artisan harness context", () => {
	it("returns deterministic bounded V1 policy for an enforceable engine channel", async () => {
		const [resolution, repeated] = await Effect.runPromise(
			Effect.gen(function* () {
				const service = yield* ArtisanHarnessContext;

				return yield* Effect.all([
					service.ResolveForEngine(descriptor("experimental")),
					service.ResolveForEngine(descriptor("experimental")),
				]);
			}).pipe(Effect.provide(ArtisanHarnessContextLive)),
		);

		expect(repeated).toEqual(resolution);
		expect(resolution).toMatchObject({
			_tag: "available",
			context: { version: artisan_harness_context_version },
		});
		if (resolution._tag === "available") {
			expect(new TextEncoder().encode(resolution.context.content).byteLength).toBeLessThan(
				4_096,
			);
			expect(resolution.context.content).toContain(
				"Never sleep, watch, shell-loop, repeatedly poll",
			);
			expect(resolution.context.content).toContain("gh run watch");
			expect(resolution.context.content).toContain("await_git_provider once");
			expect(resolution.context.content).toContain(
				"exact pull request, head commit, and gates",
			);
			expect(resolution.context.content).toContain("durably accepted");
			expect(resolution.context.content).toContain("end the run");
			expect(resolution.context.content).toContain(
				"resumes the native conversation or starts a linked follow-up run",
			);
		}
	});

	it("returns unavailable when an engine cannot enforce the policy", async () => {
		const resolution = await Effect.runPromise(
			Effect.gen(function* () {
				const service = yield* ArtisanHarnessContext;

				return yield* service.ResolveForEngine(descriptor("unsupported"));
			}).pipe(Effect.provide(ArtisanHarnessContextLive)),
		);

		expect(resolution._tag).toBe("unavailable");
	});
});

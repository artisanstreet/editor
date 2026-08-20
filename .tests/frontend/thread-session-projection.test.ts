import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import type { ThreadSessionSnapshot } from "@artisan/protocol";
import {
	ThreadSessionProjection,
	ThreadSessionProjectionLive,
} from "../../modules/frontend/src/lib/thread-interaction/session-projection";

const Session = (thread_id: string, auto_steer_enabled = true) =>
	({
		assumptions: [],
		auto_steer_enabled,
		journal_sequence: 1,
		policy: {
			engine_id: "codex",
			permission: "supervised",
			permission_mode: "on_request",
			reasoning_effort: "medium",
			sandbox_mode: "workspace_write",
			strict_clarification: false,
			web_search_enabled: false,
		},
		thread_id,
	}) satisfies ThreadSessionSnapshot;

describe("thread session projection", () => {
	it("retains authoritative route sessions in a bounded recency cache", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const services = yield* Layer.build(ThreadSessionProjectionLive);
					return yield* Effect.gen(function* () {
						const projection = yield* ThreadSessionProjection;
						for (let index = 0; index < 65; index += 1) {
							yield* projection.Publish(Session(`thread_${index}`));
						}
						yield* projection.Publish(Session("thread_1", false));
						return {
							evicted: yield* projection.Current("thread_0"),
							retained: yield* projection.Current("thread_1"),
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.evicted).toBeUndefined();
		expect(result.retained?.auto_steer_enabled).toBe(false);
	});
});

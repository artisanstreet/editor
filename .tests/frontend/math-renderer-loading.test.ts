import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { Deferred, Effect, Fiber, Layer } from "effect";
import { describe, expect, it } from "vitest";

import {
	MathRendererController,
	MakeMathRendererControllerLive,
	type ConversationMathRenderer,
} from "../../modules/frontend/src/lib/components/markdown/math-renderer-controller";

const renderer: ConversationMathRenderer = () => ({
	html: "<span>ready</span>",
	status: "rendered",
});

describe("math renderer loading", () => {
	it("coalesces concurrent callers and keeps the admitted parallel load after interruption", async () => {
		let css_requests = 0;
		let renderer_requests = 0;
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const css_reply = yield* Deferred.make<void>();
					const renderer_reply = yield* Deferred.make<ConversationMathRenderer>();
					const services = yield* Layer.build(
						MakeMathRendererControllerLive({
							LoadCss: Effect.gen(function* () {
								css_requests += 1;
								yield* Deferred.await(css_reply);
							}),
							LoadRenderer: Effect.gen(function* () {
								renderer_requests += 1;
								return yield* Deferred.await(renderer_reply);
							}),
						}),
					);
					yield* Effect.gen(function* () {
						const controller = yield* MathRendererController;
						const caller = yield* controller.Refresh.pipe(Effect.forkScoped);
						yield* controller.Refresh;
						yield* Effect.yieldNow;
						expect(css_requests).toBe(1);
						expect(renderer_requests).toBe(1);
						yield* Fiber.interrupt(caller);
						yield* Deferred.succeed(css_reply, undefined);
						yield* Deferred.succeed(renderer_reply, renderer);
						yield* Effect.yieldNow;
						yield* Effect.yieldNow;
						expect(yield* controller.Current).toEqual({
							_tag: "Ready",
							render: renderer,
						});
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result).toBeUndefined();
	});

	it("keeps its escaped fallback reachable while a retained renderer loads", () => {
		const source = readFileSync(
			resolve(
				process.cwd(),
				"modules/frontend/src/lib/components/markdown/math-renderer.svelte",
			),
			"utf8",
		);

		expect(source).toContain("renderer_controller.Current");
		expect(source).toContain("renderer_controller.Changes");
		expect(source).toContain("renderer_controller.Refresh.pipe(Effect.forkScoped)");
		expect(source).toContain('{ status: "invalid" } as const');
		expect(source).not.toContain("Effect.tryPromise");
	});
});

import { readFileSync } from "node:fs";

import { Deferred, Effect, Fiber, Layer } from "effect";
import { describe, expect, it } from "vitest";

import type { ThreadOpenSnapshot } from "@artisan/protocol";
import { ArtisanClient, ArtisanClientError } from "@artisan/transport/client";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";
import {
	ThreadOpenController,
	ThreadOpenControllerLive,
} from "../../modules/frontend/src/lib/thread-interaction/thread-open-controller";

const WithThreadId = (snapshot: ThreadOpenSnapshot, route_id: string): ThreadOpenSnapshot => {
	const thread_id = `thread_${route_id}`;
	return {
		...snapshot,
		conversation: {
			...snapshot.conversation,
			conversation_id: `conversation_${route_id}`,
			thread_id,
		},
		session: { ...snapshot.session, thread_id },
		thread: { ...snapshot.thread, thread_id },
	};
};

it("atomically admits app-owned opens while leaving only deferred waits interruptible", () => {
	const source = readFileSync(
		"modules/frontend/src/lib/thread-interaction/thread-open-controller.ts",
		"utf8",
	);
	expect(source).toContain("Effect.uninterruptibleMask((restore)");
	expect(source).toContain("Effect.forkIn(Complete(key, claim.deferred), controller_scope)");
	expect(source).toContain("restore(Deferred.await(claim.deferred))");
});

describe("thread open controller", () => {
	it("retries one unanswered cold open after transport recovery", async () => {
		const base = await Effect.runPromise(
			FixtureArtisanClientService.GetThreadOpen("thread-editor-shell"),
		);
		let reads = 0;
		const opened = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const services = yield* Layer.build(
						Layer.provide(
							ThreadOpenControllerLive,
							Layer.succeed(ArtisanClient, {
								...FixtureArtisanClientService,
								GetThreadOpen: (thread_id) =>
									Effect.gen(function* () {
										reads += 1;
										if (reads === 1) {
											return yield* Effect.fail(
												new ArtisanClientError({
													cause: new Error("request deadline exceeded"),
													code: "connection",
													message: "Forge did not answer.",
													protocol_code: "request.timeout",
													retryable: true,
												}),
											);
										}
										return WithThreadId(base, thread_id);
									}),
							}),
						),
					);
					return yield* ThreadOpenController.pipe(
						Effect.flatMap((controller) => controller.Open("101")),
						Effect.provide(services),
					);
				}),
			),
		);

		expect(reads).toBe(2);
		expect(opened.thread.thread_id).toBe("thread_101");
	});

	it("keeps cold-open work alive for followers and serves revisits from memory", async () => {
		const base = await Effect.runPromise(
			FixtureArtisanClientService.GetThreadOpen("thread-editor-shell"),
		);
		let reads = 0;
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetThreadOpen: (thread_id) =>
							Effect.gen(function* () {
								reads += 1;
								yield* Deferred.succeed(started, undefined);
								yield* Deferred.await(release);
								return WithThreadId(base, thread_id);
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(ThreadOpenControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* ThreadOpenController;
						const starter = yield* controller.Open("101").pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						const follower = yield* controller.Open("101").pipe(Effect.forkScoped);
						yield* Effect.yieldNow;
						yield* Fiber.interrupt(starter);
						yield* Deferred.succeed(release, undefined);
						const opened = yield* Fiber.join(follower);
						const revisited = yield* controller.Open("thread_101");
						return { opened, revisited };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(reads).toBe(1);
		expect(result.opened.thread.thread_id).toBe("thread_101");
		expect(result.revisited).toBe(result.opened);
	});

	/**
	 * The cap exists so the cache cannot become a second copy of the transcript
	 * store; it is set to cover a working set rather than only the back step,
	 * because re-fetching a thread visited a minute ago is what made moving
	 * between a few threads pay a skeleton every time.
	 */
	it("bounds retained conversation aggregates to the most recent threads", async () => {
		const base = await Effect.runPromise(
			FixtureArtisanClientService.GetThreadOpen("thread-editor-shell"),
		);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const services = yield* Layer.build(
						Layer.provide(
							ThreadOpenControllerLive,
							Layer.succeed(ArtisanClient, FixtureArtisanClientService),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* ThreadOpenController;
						for (const ordinal of ["1", "2", "3", "4", "5", "6", "7"]) {
							yield* controller.Publish(WithThreadId(base, ordinal));
						}
						return {
							first: yield* controller.Current("1"),
							second: yield* controller.Current("2"),
							newest: yield* controller.Current("7"),
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		/** Evicted least-recently-used once the working set is full. */
		expect(result.first).toBeUndefined();
		expect(result.second?.thread.thread_id).toBe("thread_2");
		expect(result.newest?.thread.thread_id).toBe("thread_7");
	});
});

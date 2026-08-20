import { Deferred, Effect, Exit, Fiber, Layer, Ref } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";

import { ArtisanClient, ArtisanClientError } from "@artisan/transport/client";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";
import {
	GitWorkspaceController,
	GitWorkspaceControllerLive,
	GitWorkspaceKey,
} from "../../modules/frontend/src/lib/workspace/git-workspace-controller";

const input_a = { thread_id: "thread_a", workspace_id: "workspace-artisan-editor" };
const input_b = { thread_id: "thread_b", workspace_id: "workspace-artisan-editor" };

const unavailable = new ArtisanClientError({
	cause: undefined,
	code: "connection",
	message: "Forge is not ready yet.",
	protocol_code: "forge_not_ready",
	retryable: true,
});

const FixtureWorkspace = (input: typeof input_a) =>
	FixtureArtisanClientService.GetGitWorkspace(input);

const ServicesFor = (GetGitWorkspace: typeof FixtureArtisanClientService.GetGitWorkspace) =>
	Layer.build(
		Layer.provide(
			GitWorkspaceControllerLive,
			Layer.succeed(ArtisanClient, { ...FixtureArtisanClientService, GetGitWorkspace }),
		),
	);

describe("git workspace controller", () => {
	it("coalesces overlapping remounts by the authorized thread/workspace pair", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const services = yield* ServicesFor((input) =>
						Effect.gen(function* () {
							yield* Ref.update(reads, (count) => count + 1);
							yield* Deferred.succeed(started, undefined);
							yield* Deferred.await(release);
							return yield* FixtureWorkspace(input);
						}),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* GitWorkspaceController;
						const consumers = yield* Effect.all(
							[controller.Load({ ...input_a }), controller.Load({ ...input_a })],
							{ concurrency: "unbounded" },
						).pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						expect(yield* Ref.get(reads)).toBe(1);
						yield* Deferred.succeed(release, undefined);
						return yield* Fiber.join(consumers);
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result[0]).toEqual(result[1]);
	});

	it("retains a matching projection synchronously while it refreshes", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const services = yield* ServicesFor((input) =>
						Ref.updateAndGet(reads, (count) => count + 1).pipe(
							Effect.flatMap(() => FixtureWorkspace(input)),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* GitWorkspaceController;
						yield* controller.Load(input_a);
						const retained = (yield* controller.Current).get(GitWorkspaceKey(input_a));
						yield* controller.Refresh(input_a);
						return { reads: yield* Ref.get(reads), retained };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.retained).toBeDefined();
		expect(result.reads).toBe(1);
	});

	it("expires failures, keeps admitted work alive for followers, and retries", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const release = yield* Deferred.make<void>();
					const services = yield* ServicesFor((input) =>
						Effect.gen(function* () {
							const attempt = yield* Ref.updateAndGet(reads, (count) => count + 1);
							if (attempt === 1) return yield* Effect.fail(unavailable);
							if (attempt === 2) yield* Deferred.await(release);
							return yield* FixtureWorkspace(input);
						}),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* GitWorkspaceController;
						expect(
							Exit.isFailure(yield* controller.Load(input_a).pipe(Effect.exit)),
						).toBe(true);
						const starter = yield* controller.Load(input_a).pipe(Effect.forkScoped);
						yield* Fiber.interrupt(starter);
						const follower = yield* controller.Load(input_a).pipe(Effect.forkScoped);
						yield* Deferred.succeed(release, undefined);
						return {
							workspace: yield* Fiber.join(follower),
							reads: yield* Ref.get(reads),
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.workspace).toBeDefined();
		expect(result.reads).toBe(2);
	});

	it("does not publish invalidated stale work and expires successful cache entries", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const services = yield* ServicesFor((input) =>
						Effect.gen(function* () {
							const attempt = yield* Ref.updateAndGet(reads, (count) => count + 1);
							if (attempt === 1) {
								yield* Deferred.succeed(started, undefined);
								yield* Deferred.await(release);
							}
							return yield* FixtureWorkspace(input);
						}),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* GitWorkspaceController;
						const stale = yield* controller.Load(input_a).pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						yield* controller.Invalidate(input_a);
						yield* Deferred.succeed(release, undefined);
						yield* Fiber.join(stale);
						expect((yield* controller.Current).has(GitWorkspaceKey(input_a))).toBe(
							false,
						);
						yield* controller.Load(input_b);
						yield* TestClock.adjust("31 seconds");
						yield* controller.Load(input_b);
						return yield* Ref.get(reads);
					}).pipe(Effect.provide(services));
				}),
			).pipe(Effect.provide(TestClock.layer())),
		);

		expect(result).toBe(3);
	});
});

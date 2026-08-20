import { readFile } from "node:fs/promises";

import { Deferred, Effect, Fiber, Layer, Ref, Stream } from "effect";
import { describe, expect, it } from "vitest";

import type { ThreadRetentionPolicy } from "@artisan/protocol";
import { ArtisanClient, ArtisanClientError } from "@artisan/transport/client";
import {
	ThreadRetentionPolicyController,
	ThreadRetentionPolicyControllerLive,
} from "../../modules/frontend/src/lib/settings/thread-retention-policy-controller";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";

const Policy = (inactivity_days: number): ThreadRetentionPolicy => ({
	enabled: true,
	inactivity_days,
});

describe("thread retention policy controller", () => {
	it("shares cold hydration through starter interruption and retains ready remounts", async () => {
		const reads = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const count = yield* Ref.make(0);
					const services = yield* Layer.build(
						ThreadRetentionPolicyControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									GetThreadRetentionPolicy: Effect.gen(function* () {
										yield* Ref.update(count, (n) => n + 1);
										yield* Deferred.succeed(started, undefined);
										yield* Deferred.await(release);
										return Policy(30);
									}),
								}),
							),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* ThreadRetentionPolicyController;
						const starter = yield* controller.Refresh.pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						const follower = yield* controller.Refresh.pipe(Effect.forkScoped);
						yield* Fiber.interrupt(starter);
						yield* Deferred.succeed(release, undefined);
						expect((yield* Fiber.join(follower)).inactivity_days).toBe(30);
						yield* controller.Refresh;
						return yield* Ref.get(count);
					}).pipe(Effect.provide(services));
				}),
			),
		);
		expect(reads).toBe(1);
	});

	it("publishes successful saves without readback and fences failed-save reconciliation", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reconciliation_started = yield* Deferred.make<void>();
					const release_reconciliation = yield* Deferred.make<void>();
					const reads = yield* Ref.make(0);
					let writes = 0;
					const services = yield* Layer.build(
						ThreadRetentionPolicyControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									GetThreadRetentionPolicy: Effect.gen(function* () {
										const n = yield* Ref.updateAndGet(reads, (n) => n + 1);
										if (n === 1) return Policy(10);
										yield* Deferred.succeed(reconciliation_started, undefined);
										yield* Deferred.await(release_reconciliation);
										return Policy(20);
									}),
									UpdateThreadRetentionPolicy: () => {
										writes += 1;
										return writes === 1
											? Effect.fail(
													new ArtisanClientError({
														cause: undefined,
														code: "connection",
														message: "offline",
														protocol_code: "offline",
														retryable: true,
													}),
												)
											: FixtureArtisanClientService.UpdateThreadRetentionPolicy(
													Policy(40),
												);
									},
								}),
							),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* ThreadRetentionPolicyController;
						yield* controller.Refresh;
						expect((yield* controller.Save(Policy(15)).pipe(Effect.exit))._tag).toBe(
							"Failure",
						);
						yield* Deferred.await(reconciliation_started);
						yield* controller.Save(Policy(40));
						yield* Deferred.succeed(release_reconciliation, undefined);
						yield* Effect.yieldNow;
						return { reads: yield* Ref.get(reads), state: yield* controller.Current };
					}).pipe(Effect.provide(services));
				}),
			),
		);
		expect(result.reads).toBe(2);
		expect(result.state).toEqual({ _tag: "Ready", policy: Policy(40) });
	});

	it("replaces a pre-save hydration flight when failure reconciliation needs a newer generation", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const first_started = yield* Deferred.make<void>();
					const release_first = yield* Deferred.make<void>();
					const second_started = yield* Deferred.make<void>();
					const reads = yield* Ref.make(0);
					const services = yield* Layer.build(
						ThreadRetentionPolicyControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									GetThreadRetentionPolicy: Effect.gen(function* () {
										const attempt = yield* Ref.updateAndGet(
											reads,
											(n) => n + 1,
										);
										if (attempt === 1) {
											yield* Deferred.succeed(first_started, undefined);
											yield* Deferred.await(release_first);
											return Policy(10);
										}
										yield* Deferred.succeed(second_started, undefined);
										return Policy(20);
									}),
									UpdateThreadRetentionPolicy: () =>
										Effect.fail(
											new ArtisanClientError({
												cause: undefined,
												code: "connection",
												message: "offline",
												protocol_code: "offline",
												retryable: true,
											}),
										),
								}),
							),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* ThreadRetentionPolicyController;
						const stale_hydration = yield* controller.Refresh.pipe(Effect.forkScoped);
						yield* Deferred.await(first_started);
						expect((yield* controller.Save(Policy(15)).pipe(Effect.exit))._tag).toBe(
							"Failure",
						);
						yield* Deferred.await(second_started);
						for (let attempt = 0; attempt < 100; attempt += 1) {
							const current = yield* controller.Current;
							if (current._tag === "Ready") break;
							yield* Effect.yieldNow;
						}
						const before_stale_completion = yield* controller.Current;
						yield* Deferred.succeed(release_first, undefined);
						yield* Fiber.join(stale_hydration);
						return {
							after_stale_completion: yield* controller.Current,
							before_stale_completion,
							reads: yield* Ref.get(reads),
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.reads).toBe(2);
		expect(result.before_stale_completion).toEqual({ _tag: "Ready", policy: Policy(20) });
		expect(result.after_stale_completion).toEqual({ _tag: "Ready", policy: Policy(20) });
	});

	it("publishes an unverified failed reconciliation and recovers on explicit retry", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const services = yield* Layer.build(
						ThreadRetentionPolicyControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									GetThreadRetentionPolicy: Effect.gen(function* () {
										const attempt = yield* Ref.updateAndGet(
											reads,
											(n) => n + 1,
										);
										if (attempt === 2) {
											return yield* Effect.fail(
												new ArtisanClientError({
													cause: undefined,
													code: "connection",
													message: "reconciliation failed",
													protocol_code: "offline",
													retryable: true,
												}),
											);
										}
										return Policy(attempt === 1 ? 10 : 30);
									}),
									UpdateThreadRetentionPolicy: () =>
										Effect.fail(
											new ArtisanClientError({
												cause: undefined,
												code: "connection",
												message: "save failed",
												protocol_code: "offline",
												retryable: true,
											}),
										),
								}),
							),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* ThreadRetentionPolicyController;
						yield* controller.Refresh;
						const reconciled_failure = yield* Deferred.make<void>();
						let unverified_updates = 0;
						yield* controller.Changes.pipe(
							Stream.runForEach((current) =>
								Effect.gen(function* () {
									if (current._tag !== "Unverified") return;
									unverified_updates += 1;
									if (unverified_updates === 2) {
										yield* Deferred.succeed(reconciled_failure, undefined);
									}
								}),
							),
							Effect.forkScoped,
						);
						yield* controller.Save(Policy(20)).pipe(Effect.exit);
						yield* Deferred.await(reconciled_failure);
						const failed_state = yield* controller.Current;
						const recovered = yield* controller.Refresh;
						return {
							failed_state,
							reads: yield* Ref.get(reads),
							recovered,
							state: yield* controller.Current,
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.failed_state).toEqual({ _tag: "Unverified" });
		expect(result.reads).toBe(3);
		expect(result.recovered).toEqual(Policy(30));
		expect(result.state).toEqual({ _tag: "Ready", policy: Policy(30) });
	});

	it("routes the Settings screen through the app-scoped retained controller", async () => {
		const [runtime, threads] = await Promise.all([
			readFile("modules/frontend/src/lib/runtime/browser-frontend-runtime.ts", "utf8"),
			readFile("modules/frontend/src/routes/components/settings/threads.svelte", "utf8"),
		]);

		expect(runtime).toContain("ThreadRetentionPolicyControllerLive");
		expect(threads).toContain("retention_controller.Current");
		expect(threads).toContain("retention_controller.Changes");
		expect(threads).toContain('initial_policy_state._tag !== "Ready"');
		expect(threads).not.toContain("ArtisanClient");
		expect(threads).not.toContain("GetThreadRetentionPolicy");
		expect(threads).not.toContain("UpdateThreadRetentionPolicy");
	});
});

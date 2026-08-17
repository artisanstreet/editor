import { Effect, Exit, Layer, Option, Queue, Ref, Scope, Stream } from "effect";
import { describe, expect, it } from "vitest";

import { MakeSnowflakeIdLive } from "@artisan/protocol";
import {
	ArtisanClient,
	ArtisanClientError,
	type SurfaceUsageAggregateUpdate,
} from "@artisan/transport/client";
import {
	RunUsageController,
	RunUsageControllerLive,
} from "../../modules/frontend/src/lib/context-usage/run-usage-controller";
import {
	DraftThreadController,
	DraftThreadControllerLive,
} from "../../modules/frontend/src/lib/root/draft-thread";
import {
	FixtureArtisanClientService,
	fixture_project,
} from "../../modules/frontend/src/lib/runtime/fixtures/client";

const submission = { attachments: [], text: "Keep this exact first message." };

const policy = {
	engine_id: "codex" as const,
	model: "gpt-5.6-codex",
	permission: "supervised" as const,
	permission_mode: "on_request" as const,
	reasoning_effort: "medium" as const,
	sandbox_mode: "workspace_write" as const,
	service_tier: "standard" as const,
	strict_clarification: false,
	web_search_enabled: false,
};

const ClientFailure = (message: string) =>
	new ArtisanClientError({
		cause: undefined,
		code: "protocol",
		message,
		protocol_code: "test_failure",
		retryable: false,
	});

describe("frontend controller hostile races", () => {
	it("retains one draft command through a failed send and an interrupted remount claim", async () => {
		let policy_attempts = 0;
		const client_layer = Layer.succeed(ArtisanClient, {
			...FixtureArtisanClientService,
			UpdateThreadSessionPolicy: (input) =>
				Effect.gen(function* () {
					policy_attempts += 1;
					if (policy_attempts === 1)
						return yield* Effect.fail(ClientFailure("policy write lost"));
					return yield* FixtureArtisanClientService.UpdateThreadSessionPolicy(input);
				}),
		});
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* DraftThreadController;
				yield* controller.Initialize(fixture_project, policy);

				const failed = yield* Effect.exit(controller.Submit(submission));
				expect(failed._tag).toBe("Failure");
				const after_failure = yield* controller.Current;
				expect(after_failure._tag).toBe("Created");
				if (after_failure._tag !== "Created") return yield* Effect.die("draft was lost");

				const retried = yield* controller.Retry;
				expect(retried.command_id).toBe(after_failure.command_id);

				const first_route_scope = yield* Scope.make();
				const owner = yield* controller
					.AwaitPendingSubmissionClaim(retried.thread_id)
					.pipe(Scope.provide(first_route_scope));
				if (owner === undefined) return yield* Effect.die("no claim owner");

				const replacement_route_scope = yield* Scope.make();
				const blocked_replacement = yield* controller
					.AwaitPendingSubmissionClaim(retried.thread_id)
					.pipe(
						Scope.provide(replacement_route_scope),
						Effect.timeoutOption("20 millis"),
					);
				expect(Option.isNone(blocked_replacement)).toBe(true);

				/** Closing the outgoing route triggers the claim's controller-owned finalizer. */
				yield* Scope.close(first_route_scope, Exit.void);
				const remounted_owner = yield* controller
					.AwaitPendingSubmissionClaim(retried.thread_id)
					.pipe(Scope.provide(replacement_route_scope));
				expect(remounted_owner?.command_id).toBe(owner.command_id);
				if (remounted_owner === undefined) return yield* Effect.die("remount lost claim");

				/** A stale duplicate route close cannot release the replacement claim. */
				yield* Scope.close(first_route_scope, Exit.void);
				const contender_route_scope = yield* Scope.make();
				const blocked_contender = yield* controller
					.AwaitPendingSubmissionClaim(retried.thread_id)
					.pipe(
						Scope.provide(contender_route_scope),
						Effect.timeoutOption("20 millis"),
					);
				yield* Scope.close(contender_route_scope, Exit.void);
				expect(Option.isNone(blocked_contender)).toBe(true);

				yield* remounted_owner.Complete;
				yield* Scope.close(replacement_route_scope, Exit.void);
				expect((yield* controller.Current)._tag).toBe("Uninitialized");
			}).pipe(
				Effect.provide(DraftThreadControllerLive),
				Effect.provide(MakeSnowflakeIdLive(17).pipe(Layer.orDie)),
				Effect.provide(client_layer),
			),
		);

		expect(result).toBeUndefined();
	});

	it("keeps B selected when A releases late and A's retired stream emits afterwards", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const a_updates = yield* Queue.unbounded<SurfaceUsageAggregateUpdate>();
					const b_updates = yield* Queue.unbounded<SurfaceUsageAggregateUpdate>();
					const observed = yield* Ref.make<unknown>({ _tag: "None" });
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						SubscribeSurfaceUsageAggregate: (input) =>
							Effect.gen(function* () {
								yield* Effect.void;
								return Stream.fromQueue(
									input.scope_id === "run-a" ? a_updates : b_updates,
								);
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(RunUsageControllerLive, client_layer),
					);
					yield* Effect.gen(function* () {
						const controller = yield* RunUsageController;
						yield* Stream.runForEach(controller.Changes, (state) =>
							Effect.gen(function* () {
								yield* Ref.set(observed, state);
							}),
						).pipe(Effect.forkScoped);

						const a = yield* controller.Acquire("run-a");
						const b = yield* controller.Acquire("run-b");
						yield* a.Release;
						yield* Queue.offer(b_updates, {
							type: "snapshot",
							snapshot: {
								aggregate: { scope: "run", scope_id: "run-b", input_tokens: 2 },
								journal_sequence: 2,
							},
						});
						/** The retired stream emits last; owner/run guard must reject it. */
						yield* Queue.offer(a_updates, {
							type: "snapshot",
							snapshot: {
								aggregate: { scope: "run", scope_id: "run-a", input_tokens: 1 },
								journal_sequence: 3,
							},
						});
						yield* Effect.yieldNow;
						yield* Effect.yieldNow;

						expect(yield* Ref.get(observed)).toMatchObject({
							_tag: "Ready",
							run_id: "run-b",
							aggregate: { input_tokens: 2, scope_id: "run-b" },
						});
						yield* b.Release;
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result).toBeUndefined();
	});
});

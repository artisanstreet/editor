import { Effect, Exit, Layer, Option, Scope } from "effect";
import { describe, expect, it } from "vitest";

import { MakeSnowflakeIdLive } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";
import {
	DraftThreadController,
	DraftThreadControllerLive,
} from "../../modules/frontend/src/lib/root/draft-thread";
import {
	FixtureArtisanClientService,
	fixture_project,
} from "../../modules/frontend/src/lib/runtime/fixtures/client";

const policy = {
	engine_id: "codex" as const,
	model: "gpt-5.6-sol",
	permission: "supervised" as const,
	permission_mode: "on_request" as const,
	reasoning_effort: "xhigh" as const,
	sandbox_mode: "workspace_write" as const,
	service_tier: "fast" as const,
	strict_clarification: false,
	web_search_enabled: false,
};

const submission = { attachments: [], text: "Keep this first message." };

describe("new-thread first submission recovery", () => {
	it("re-offers an acquired claim after its route scope closes", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const controller = yield* DraftThreadController;
					yield* controller.Initialize(fixture_project, policy);
					const created = yield* controller.Submit(submission);

					const interrupted_route_scope = yield* Scope.make();
					const first = yield* controller
						.AwaitPendingSubmissionClaim(created.thread_id)
						.pipe(Scope.provide(interrupted_route_scope));
					yield* Scope.close(interrupted_route_scope, Exit.void);

					const replacement_route_scope = yield* Scope.make();
					const reoffered = yield* controller
						.AwaitPendingSubmissionClaim(created.thread_id)
						.pipe(
							Scope.provide(replacement_route_scope),
							Effect.timeoutOption("2 seconds"),
						);
					const replacement = Option.getOrUndefined(reoffered);
					if (replacement !== undefined) yield* replacement.Complete;
					yield* Scope.close(replacement_route_scope, Exit.void);

					return { created, first, replacement, settled: yield* controller.Current };
				}).pipe(
					Effect.provide(DraftThreadControllerLive),
					Effect.provide(MakeSnowflakeIdLive(29).pipe(Layer.orDie)),
					Effect.provide(Layer.succeed(ArtisanClient, FixtureArtisanClientService)),
				),
			),
		);

		expect(result.first?.submission).toEqual(submission);
		expect(result.replacement?.command_id).toBe(result.created.command_id);
		expect(result.replacement?.submission).toEqual(submission);
		expect(result.settled).toEqual({ _tag: "Uninitialized" });
	});

	it("completes a delivered submission even after its claim was released", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const controller = yield* DraftThreadController;
					yield* controller.Initialize(fixture_project, policy);
					const created = yield* controller.Submit(submission);

					const route_scope = yield* Scope.make();
					const claim = yield* controller
						.AwaitPendingSubmissionClaim(created.thread_id)
						.pipe(Scope.provide(route_scope));
					yield* Scope.close(route_scope, Exit.void);
					if (claim !== undefined) yield* claim.Complete;

					return { claim, settled: yield* controller.Current };
				}).pipe(
					Effect.provide(DraftThreadControllerLive),
					Effect.provide(MakeSnowflakeIdLive(29).pipe(Layer.orDie)),
					Effect.provide(Layer.succeed(ArtisanClient, FixtureArtisanClientService)),
				),
			),
		);

		expect(result.claim?.submission).toEqual(submission);
		expect(result.settled).toEqual({ _tag: "Uninitialized" });
	});
});

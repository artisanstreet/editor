import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { Deferred, Effect, Fiber, Layer } from "effect";
import { describe, expect, it } from "vitest";

import type { TerminalSession } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";
import {
	ThreadTerminalsController,
	ThreadTerminalsControllerLive,
} from "../../modules/frontend/src/lib/terminal/thread-terminals-controller";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";

const terminal: TerminalSession = {
	args: [],
	cols: 80,
	created_at: "2026-08-14T00:00:00.000Z",
	executable: "pnpm",
	generation: 1,
	rows: 24,
	state: "active",
	terminal_id: "terminal_fixture",
	thread_id: "thread_fixture",
	updated_at: "2026-08-14T00:00:00.000Z",
	working_directory: "fixture",
	workspace_id: "workspace_fixture",
};

describe("thread terminals loading", () => {
	it("coalesces a retained list request after the admitting caller is interrupted", async () => {
		let requests = 0;
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reply = yield* Deferred.make<ReadonlyArray<TerminalSession>>();
					const services = yield* Layer.build(
						ThreadTerminalsControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									ListTerminals: () =>
										Effect.gen(function* () {
											requests += 1;
											return yield* Deferred.await(reply);
										}),
								}),
							),
						),
					);
					yield* Effect.gen(function* () {
						const controller = yield* ThreadTerminalsController;
						const caller = yield* controller
							.Refresh(terminal.thread_id, terminal.workspace_id)
							.pipe(Effect.forkScoped);
						yield* controller.Refresh(terminal.thread_id, terminal.workspace_id);
						yield* Effect.yieldNow;
						expect(requests).toBe(1);
						yield* Fiber.interrupt(caller);
						yield* Deferred.succeed(reply, [terminal]);
						yield* Effect.yieldNow;
						yield* Effect.yieldNow;
						expect(
							yield* controller.Current(terminal.thread_id, terminal.workspace_id),
						).toEqual({
							_tag: "Ready",
							terminals: [terminal],
							thread_id: terminal.thread_id,
							workspace_id: terminal.workspace_id,
						});
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result).toBeUndefined();
	});

	it("starts list and output work in scoped background fibers", () => {
		const source = readFileSync(
			resolve(
				process.cwd(),
				"modules/frontend/src/routes/components/thread-terminals-card.svelte",
			),
			"utf8",
		);

		expect(source).toContain("terminals_controller.Current(thread_id, workspace_id)");
		expect(source).toContain(
			"terminals_controller.Refresh(next_thread_id, next_workspace_id).pipe",
		);
		expect(source).toContain("const DetachOutputScope = () => {");
		expect(source).toContain("Scope.close(scope, Exit.void)");
		expect(source).toContain("const next_output_scope = yield* Scope.fork(component_scope);");
		expect(source).toContain(
			"yield* Effect.forkIn(FollowOutput(terminal, generation), next_output_scope);",
		);
		expect(source).toContain("yield* CloseOutputScope(previous_scope);");
		expect(source).toContain("CloseOutputScope(previous_scope).pipe(Effect.forkScoped)");
		expect(source).toContain("onOpenChange={yield* HandleViewerOpenChange(event)}");
		expect(source).toContain("if (generation !== output_generation) return;");
		expect(source).not.toContain("yield* LoadTerminals;");
		expect(source).not.toContain("yield* FollowOutput(viewing);");
		expect(source).not.toContain("FollowOutput(terminal, generation).pipe(Effect.forkScoped)");
	});
});

import { Deferred, Effect, Layer, ManagedRuntime, Option, Ref } from "effect";
import { describe, expect, it } from "vitest";

import { WorkspaceFileService } from "../../modules/backend/src/workspace/workspace-file-service";
import {
	WorkspaceReplaceApprovalCoordinator,
	WorkspaceReplaceApprovalCoordinatorLive,
} from "../../modules/backend/src/workspace/workspace-replace-approval-coordinator";
import {
	WorkspaceReplaceApprovalRepository,
	type WorkspaceReplaceApprovalAcceptance,
	type WorkspaceReplaceApprovalDecision,
} from "../../modules/backend/src/workspace/workspace-replace-approval-repository";

const decision: WorkspaceReplaceApprovalDecision = {
	approval_id: "approval_1",
	approved: true,
	message_id: "decision_1",
	sent_at: "2026-07-13T12:00:00.000Z",
	thread_id: "thread_1",
};

function acceptance(status: "accepted" | "duplicate"): WorkspaceReplaceApprovalAcceptance {
	return { status } as WorkspaceReplaceApprovalAcceptance;
}

function make_runtime(
	approvals: typeof WorkspaceReplaceApprovalRepository.Service,
	workspace_files: typeof WorkspaceFileService.Service,
) {
	const dependencies = Layer.mergeAll(
		Layer.succeed(WorkspaceReplaceApprovalRepository, approvals),
		Layer.succeed(WorkspaceFileService, workspace_files),
	);

	return ManagedRuntime.make(
		WorkspaceReplaceApprovalCoordinatorLive.pipe(Layer.provide(dependencies)),
	);
}

describe("workspace replace approval coordinator", () => {
	it("recovers denied cleanup and every executable approval at startup", async () => {
		const dispatched = await Effect.runPromise(Deferred.make<void>());
		const actions: string[] = [];
		const runtime = make_runtime(
			{
				Decide: () => Effect.die("unused"),
				ListDeniedUnsettled: Effect.succeed(["approval_denied"]),
				ListExecutable: Effect.succeed(["approval_approved", "approval_executing"]),
				MarkApplied: () => Effect.die("unused"),
				MarkExecuting: () => Effect.die("unused"),
				MarkRejected: () => Effect.die("unused"),
				Query: () => Effect.die("unused"),
				ReadByMessage: () => Effect.die("unused"),
				ReadDenied: (approval_id) =>
					Effect.succeed({
						approval: { thread_id: "thread_denied" },
						approval_id,
					} as never),
				ReadExecution: (approval_id) =>
					Effect.succeed({
						approval: { thread_id: "thread_executable" },
						approval_id,
					} as never),
				Request: () => Effect.die("unused"),
			} as typeof WorkspaceReplaceApprovalRepository.Service,
			{
				ExecuteApproved: (approval_id) =>
					Effect.gen(function* () {
						actions.push(`execute:${approval_id}`);

						if (actions.length === 3) {
							yield* Deferred.succeed(dispatched, undefined);
						}
					}),
				Read: () => Effect.die("unused"),
				Replace: () => Effect.die("unused"),
				Review: () => Effect.die("unused"),
				Rollback: () => Effect.die("unused"),
				SettleDeniedApproval: (approval_id) =>
					Effect.sync(() => actions.push(`deny:${approval_id}`)),
			} as typeof WorkspaceFileService.Service,
		);

		try {
			await runtime.runPromise(Effect.service(WorkspaceReplaceApprovalCoordinator));
			await Effect.runPromise(Deferred.await(dispatched));

			expect(actions).toEqual([
				"deny:approval_denied",
				"execute:approval_approved",
				"execute:approval_executing",
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("persists a response, wakes execution, and does not re-execute an exact duplicate", async () => {
		const executed = await Effect.runPromise(Deferred.make<void>());
		const executable = await Effect.runPromise(Ref.make(false));
		const decisions: WorkspaceReplaceApprovalDecision[] = [];
		const executions: string[] = [];
		const runtime = make_runtime(
			{
				Decide: (input) =>
					Effect.gen(function* () {
						decisions.push(input);
						const is_duplicate = decisions.length > 1;

						if (!is_duplicate) {
							yield* Ref.set(executable, true);
						}

						return acceptance(is_duplicate ? "duplicate" : "accepted");
					}),
				ListDeniedUnsettled: Effect.succeed([]),
				ListExecutable: Ref.get(executable).pipe(
					Effect.map((ready) => (ready ? [decision.approval_id] : [])),
				),
				MarkApplied: () => Effect.die("unused"),
				MarkExecuting: () => Effect.die("unused"),
				MarkRejected: () => Effect.die("unused"),
				Query: () => Effect.die("unused"),
				ReadByMessage: () => Effect.die("unused"),
				ReadDenied: () => Effect.die("unused"),
				ReadExecution: (approval_id) =>
					Effect.succeed({
						approval: { thread_id: decision.thread_id },
						approval_id,
					} as never),
				Request: () => Effect.die("unused"),
			} as typeof WorkspaceReplaceApprovalRepository.Service,
			{
				ExecuteApproved: (approval_id) =>
					Effect.gen(function* () {
						executions.push(approval_id);
						yield* Ref.set(executable, false);
						yield* Deferred.succeed(executed, undefined);
					}),
				Read: () => Effect.die("unused"),
				Replace: () => Effect.die("unused"),
				Review: () => Effect.die("unused"),
				Rollback: () => Effect.die("unused"),
				SettleDeniedApproval: () => Effect.die("unused"),
			} as typeof WorkspaceFileService.Service,
		);

		try {
			const accepted = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* WorkspaceReplaceApprovalCoordinator;

					return yield* coordinator.Respond(decision);
				}),
			);

			await Effect.runPromise(Deferred.await(executed));
			const duplicate = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* WorkspaceReplaceApprovalCoordinator;

					return yield* coordinator.Respond(decision);
				}),
			);

			expect(accepted.status).toBe("accepted");
			expect(duplicate.status).toBe("duplicate");
			expect(decisions).toEqual([decision, decision]);
			expect(executions).toEqual([decision.approval_id]);
		} finally {
			await runtime.dispose();
		}
	});

	it("drains an admitted file action before quiescing and blocks later dispatch for its thread", async () => {
		const action_started = await Effect.runPromise(Deferred.make<void>());
		const allow_action_completion = await Effect.runPromise(Deferred.make<void>());
		const quiesced = await Effect.runPromise(Deferred.make<void>());
		const later_read = await Effect.runPromise(Deferred.make<void>());
		const executable = await Effect.runPromise(
			Ref.make<ReadonlyArray<string>>(["approval_active"]),
		);
		const executions: string[] = [];
		const runtime = make_runtime(
			{
				Decide: () => Effect.die("unused"),
				ListDeniedUnsettled: Effect.succeed([]),
				ListExecutable: Ref.get(executable),
				MarkApplied: () => Effect.die("unused"),
				MarkExecuting: () => Effect.die("unused"),
				MarkRejected: () => Effect.die("unused"),
				Query: () => Effect.die("unused"),
				ReadByMessage: () => Effect.die("unused"),
				ReadDenied: () => Effect.die("unused"),
				ReadExecution: (approval_id) =>
					approval_id === "approval_later"
						? Deferred.succeed(later_read, undefined).pipe(
								Effect.as({
									approval: { thread_id: decision.thread_id },
									approval_id,
								} as never),
							)
						: Effect.succeed({
								approval: { thread_id: decision.thread_id },
								approval_id,
							} as never),
				Request: () => Effect.die("unused"),
			} as typeof WorkspaceReplaceApprovalRepository.Service,
			{
				ExecuteApproved: (approval_id) =>
					Effect.gen(function* () {
						executions.push(approval_id);

						if (approval_id === "approval_active") {
							yield* Deferred.succeed(action_started, undefined);
							yield* Deferred.await(allow_action_completion);
						}
					}),
				Read: () => Effect.die("unused"),
				Replace: () => Effect.die("unused"),
				Review: () => Effect.die("unused"),
				Rollback: () => Effect.die("unused"),
				SettleDeniedApproval: () => Effect.die("unused"),
			} as typeof WorkspaceFileService.Service,
		);

		try {
			await runtime.runPromise(Effect.service(WorkspaceReplaceApprovalCoordinator));
			await Effect.runPromise(Deferred.await(action_started));
			const quiesce = runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* WorkspaceReplaceApprovalCoordinator;

					yield* coordinator.QuiesceThread(decision.thread_id);
					yield* Deferred.succeed(quiesced, undefined);
				}),
			);

			expect(await Effect.runPromise(Deferred.poll(quiesced))).toEqual(Option.none());
			await Effect.runPromise(Deferred.succeed(allow_action_completion, undefined));
			await Effect.runPromise(Deferred.await(quiesced));
			await Effect.runPromise(Ref.set(executable, ["approval_later"]));
			await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* WorkspaceReplaceApprovalCoordinator;

					yield* coordinator.Recover;
				}),
			);
			await Effect.runPromise(Deferred.await(later_read));

			expect(executions).toEqual(["approval_active"]);
			await quiesce;
		} finally {
			await runtime.dispose();
		}
	});
});

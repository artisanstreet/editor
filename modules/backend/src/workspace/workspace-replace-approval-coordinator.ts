import { Context, Effect, Exit, Layer, Ref, Scope } from "effect";

import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";
import { WorkspaceFileService } from "./workspace-file-service";
import {
	WorkspaceReplaceApprovalRepository,
	type WorkspaceReplaceApprovalAcceptance,
	type WorkspaceReplaceApprovalDecision,
	type WorkspaceReplaceApprovalRepositoryError,
} from "./workspace-replace-approval-repository";

type DispatchState = "idle" | "pending" | "running";

/** Coordinates durable approval decisions with recoverable workspace replacement execution. */
export class WorkspaceReplaceApprovalCoordinator extends Context.Service<
	WorkspaceReplaceApprovalCoordinator,
	{
		readonly QuiesceThread: (thread_id: string) => Effect.Effect<void>;
		readonly Recover: Effect.Effect<void>;
		readonly Respond: (
			input: WorkspaceReplaceApprovalDecision,
		) => Effect.Effect<
			WorkspaceReplaceApprovalAcceptance,
			WorkspaceReplaceApprovalRepositoryError
		>;
	}
>()("Artisan/WorkspaceReplaceApprovalCoordinator") {}

/** Builds the scoped dispatcher that resumes approved and denied replacements from SQLite. */
export const WorkspaceReplaceApprovalCoordinatorLive = Layer.effect(
	WorkspaceReplaceApprovalCoordinator,
	Effect.gen(function* () {
		const approvals = yield* WorkspaceReplaceApprovalRepository;
		const workspace_files = yield* WorkspaceFileService;
		const service_scope = yield* Scope.make();
		const dispatch_state = yield* Ref.make<DispatchState>("idle");
		const dispatch_fence = yield* MakeThreadDispatchFence;

		yield* Effect.addFinalizer(() => Scope.close(service_scope, Exit.succeed(undefined)));

		const DispatchPending = Effect.gen(function* () {
			const denied = yield* approvals.ListDeniedUnsettled;
			const executable = yield* approvals.ListExecutable;
			const denied_results = yield* Effect.forEach(denied, (approval_id) =>
				Effect.gen(function* () {
					const binding = yield* approvals.ReadDenied(approval_id);

					yield* dispatch_fence.Run(
						binding.approval.thread_id,
						workspace_files.SettleDeniedApproval(approval_id),
					);
				}).pipe(Effect.exit),
			);
			const executable_results = yield* Effect.forEach(executable, (approval_id) =>
				Effect.gen(function* () {
					const execution = yield* approvals.ReadExecution(approval_id);

					yield* dispatch_fence.Run(
						execution.approval.thread_id,
						workspace_files.ExecuteApproved(approval_id),
					);
				}).pipe(Effect.exit),
			);

			return [...denied_results, ...executable_results].some(Exit.isFailure);
		});

		const DispatchLoop = Effect.gen(function* () {
			while (true) {
				const result = yield* DispatchPending.pipe(Effect.exit);
				const retry = Exit.isFailure(result) || result.value;
				const continue_dispatch = yield* Ref.modify(dispatch_state, (state) => {
					const requested = state === "pending";
					const continue_running = retry || requested;

					return [continue_running, continue_running ? "running" : "idle"] as const;
				});

				if (!continue_dispatch) {
					return;
				}

				if (retry) {
					yield* Effect.sleep("1 second");
				}
			}
		});

		const WakeDispatcher = Effect.gen(function* () {
			const start = yield* Ref.modify(dispatch_state, (state) =>
				state === "idle" ? ([true, "running"] as const) : ([false, "pending"] as const),
			);

			if (start) {
				yield* Effect.forkIn(DispatchLoop, service_scope);
			}
		});

		const Respond = (input: WorkspaceReplaceApprovalDecision) =>
			Effect.gen(function* () {
				const acceptance = yield* approvals.Decide(input);

				yield* WakeDispatcher;

				return acceptance;
			});
		const Recover = WakeDispatcher;
		const QuiesceThread = (thread_id: string) => dispatch_fence.Quiesce(thread_id, Effect.void);

		yield* Recover;

		return { QuiesceThread, Recover, Respond };
	}),
);

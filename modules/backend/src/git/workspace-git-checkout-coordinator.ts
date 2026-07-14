import {
	Context,
	Crypto,
	Data,
	Effect,
	Encoding,
	Exit,
	Layer,
	Option,
	Ref,
	Result,
	Scope,
	Schema,
} from "effect";

import {
	Identifier,
	IsoDateTime,
	WorkspaceGitCheckoutApprovalResponseRequest,
	WorkspaceGitCheckoutRequest,
} from "@artisan/protocol";

import {
	WorkspaceGitCheckoutConflict,
	WorkspaceGitCheckoutRepository,
	type WorkspaceGitCheckoutAcceptance,
	type WorkspaceGitCheckoutExecution,
	type WorkspaceGitCheckoutRepositoryError,
} from "./workspace-git-checkout-repository";
import {
	WorkspaceGitSessionService,
	type WorkspaceGitSessionServiceError,
} from "./workspace-git-session-service";
import {
	WorkspaceGitObserver,
	type WorkspaceGitObservation,
	WorkspaceGitObservationError,
} from "./workspace-git-observer";
import { WorkspaceGitRegistry } from "./workspace-git-registry";
import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";

const CheckoutRequestInput = Schema.Struct({
	...WorkspaceGitCheckoutRequest.fields,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

const CheckoutDecisionInput = Schema.Struct({
	...WorkspaceGitCheckoutApprovalResponseRequest.fields,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

/** Supplies one explicit checkout approval request with durable command metadata. */
export type WorkspaceGitCheckoutRequestInput = typeof CheckoutRequestInput.Type;

/** Supplies one explicit checkout approval response with durable command metadata. */
export type WorkspaceGitCheckoutDecisionInput = typeof CheckoutDecisionInput.Type;

/** Reports request validation or native Git failures without exposing process output. */
export class WorkspaceGitCheckoutFailure extends Data.TaggedError("WorkspaceGitCheckoutFailure")<{
	readonly reason: "git_failed" | "invalid_request" | "no_change" | "target_missing";
}> {}

/** Represents synchronous failures surfaced by the checkout coordinator. */
export type WorkspaceGitCheckoutCoordinatorError =
	| WorkspaceGitCheckoutFailure
	| WorkspaceGitCheckoutRepositoryError
	| WorkspaceGitSessionServiceError;

/** Coordinates approval, one-shot branch mutation, recovery, and durable projection updates. */
export class WorkspaceGitCheckoutCoordinator extends Context.Service<
	WorkspaceGitCheckoutCoordinator,
	{
		readonly QuiesceThread: (thread_id: string) => Effect.Effect<void>;
		readonly Recover: Effect.Effect<void, WorkspaceGitCheckoutCoordinatorError>;
		readonly Request: (
			input: WorkspaceGitCheckoutRequestInput,
		) => Effect.Effect<WorkspaceGitCheckoutAcceptance, WorkspaceGitCheckoutCoordinatorError>;
		readonly Respond: (
			input: WorkspaceGitCheckoutDecisionInput,
		) => Effect.Effect<WorkspaceGitCheckoutAcceptance, WorkspaceGitCheckoutCoordinatorError>;
	}
>()("Artisan/WorkspaceGitCheckoutCoordinator") {}

type DispatchState = "idle" | "pending" | "running";

function checkout_fingerprint(input: WorkspaceGitCheckoutRequestInput) {
	return JSON.stringify({
		expected_session_version: input.expected_session_version,
		message_id: input.message_id,
		sent_at: input.sent_at,
		target_branch: input.target_branch,
		thread_id: input.thread_id,
		workspace_id: input.workspace_id,
	});
}

function request_matches_existing(
	input: WorkspaceGitCheckoutRequestInput,
	acceptance: WorkspaceGitCheckoutAcceptance,
) {
	const approval = acceptance.approval;

	return (
		approval.created_at === input.sent_at &&
		approval.expected_session_version === input.expected_session_version &&
		approval.source_command_id === input.message_id &&
		approval.target_branch === input.target_branch &&
		approval.thread_id === input.thread_id &&
		approval.workspace_id === input.workspace_id
	);
}

function observation_matches_source(
	observation: WorkspaceGitObservation,
	execution: WorkspaceGitCheckoutExecution,
) {
	return (
		observation.branch === execution.approval.source_branch &&
		observation.has_diff === false &&
		observation.head === execution.approval.source_head &&
		observation.repository_root === execution.repository_root &&
		observation.selected_worktree_path === execution.selected_worktree_path &&
		observation.state === "ready"
	);
}

function observation_matches_target(
	observation: WorkspaceGitObservation,
	execution: WorkspaceGitCheckoutExecution,
) {
	return (
		observation.branch === execution.approval.target_branch &&
		observation.has_diff === false &&
		observation.head === execution.target_head &&
		observation.repository_root === execution.repository_root &&
		observation.selected_worktree_path === execution.selected_worktree_path &&
		observation.state === "ready"
	);
}

/** Supplies the scoped dispatcher that never retries a native checkout process. */
export const WorkspaceGitCheckoutCoordinatorLive = Layer.effect(
	WorkspaceGitCheckoutCoordinator,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const observer = yield* WorkspaceGitObserver;
		const registry = yield* WorkspaceGitRegistry;
		const repository = yield* WorkspaceGitCheckoutRepository;
		const sessions = yield* WorkspaceGitSessionService;
		const dispatch_fence = yield* MakeThreadDispatchFence;
		const dispatch_state = yield* Ref.make<DispatchState>("idle");
		const service_scope = yield* Scope.make();

		yield* Effect.addFinalizer(() => Scope.close(service_scope, Exit.succeed(undefined)));

		const Fingerprint = (input: WorkspaceGitCheckoutRequestInput) =>
			crypto.digest("SHA-256", new TextEncoder().encode(checkout_fingerprint(input))).pipe(
				Effect.map(Encoding.encodeHex),
				Effect.mapError(
					() => new WorkspaceGitCheckoutFailure({ reason: "invalid_request" }),
				),
			);
		const Project = (
			execution: WorkspaceGitCheckoutExecution,
			suffix: "failure" | "post" | "preflight" | "recovery",
			observation?: WorkspaceGitObservation,
		) => {
			const input = {
				kind: suffix === "recovery" ? "recovery" : "checkout",
				operation_id: `workspace_git_checkout:${execution.approval.approval_id}:${suffix}`,
				sent_at: execution.approval.updated_at,
				thread_id: execution.approval.thread_id,
				workspace_id: execution.approval.workspace_id,
			} as const;

			return observation === undefined
				? sessions.Project(input)
				: sessions.ProjectObserved(input, observation);
		};
		const SettleObservedFailure = (
			execution: WorkspaceGitCheckoutExecution,
			suffix: "failure" | "preflight",
			terminal: "rejected" | "unknown",
			observation: WorkspaceGitObservation,
		) =>
			Effect.gen(function* () {
				const projection = yield* Project(execution, suffix, observation).pipe(
					Effect.result,
				);

				yield* terminal === "rejected"
					? repository.MarkRejected(execution.approval.approval_id)
					: repository.MarkUnknown(execution.approval.approval_id);

				if (Result.isFailure(projection)) {
					return yield* Effect.fail(projection.failure);
				}
			});
		const ExecuteApproved = (approval_id: string) =>
			Effect.gen(function* () {
				const execution_claim = yield* repository
					.MarkExecuting(approval_id)
					.pipe(Effect.result);

				if (Result.isFailure(execution_claim)) {
					if (
						execution_claim.failure instanceof WorkspaceGitCheckoutConflict &&
						(execution_claim.failure.reason === "session_dirty" ||
							execution_claim.failure.reason === "session_stale")
					) {
						yield* repository.MarkRejected(approval_id);

						return;
					}

					return yield* Effect.fail(execution_claim.failure);
				}

				if (execution_claim.success.status === "duplicate") {
					return;
				}

				const execution = yield* repository.ReadExecution(approval_id);
				const capability = yield* registry
					.Get(execution.approval.workspace_id)
					.pipe(
						Effect.mapError(
							() => new WorkspaceGitCheckoutFailure({ reason: "git_failed" }),
						),
					);
				const before = yield* observer.Observe(execution.approval.workspace_id).pipe(
					Effect.catch(() =>
						Effect.gen(function* () {
							yield* repository.MarkUnknown(approval_id);

							return yield* Effect.fail(
								new WorkspaceGitObservationError({ reason: "git_failed" }),
							);
						}),
					),
				);

				if (!observation_matches_source(before, execution)) {
					yield* SettleObservedFailure(execution, "preflight", "rejected", before);

					return;
				}

				const target_head = yield* capability.read
					.ResolveLocalBranch(execution.approval.target_branch)
					.pipe(
						Effect.catch(() =>
							Effect.gen(function* () {
								yield* repository.MarkRejected(approval_id);

								return yield* Effect.fail(
									new WorkspaceGitCheckoutFailure({ reason: "git_failed" }),
								);
							}),
						),
					);

				if (Option.getOrUndefined(target_head) !== execution.target_head) {
					yield* repository.MarkRejected(approval_id);

					return;
				}

				const prepared = yield* capability.mutation
					.Prepare({
						target_branch: execution.approval.target_branch,
						type: "checkout",
					})
					.pipe(Effect.result);

				if (
					Result.isFailure(prepared) ||
					prepared.success.type !== "checkout" ||
					prepared.success.source.branch !== execution.approval.source_branch ||
					prepared.success.source.head !== execution.approval.source_head ||
					prepared.success.target_head !== execution.target_head
				) {
					yield* SettleObservedFailure(execution, "preflight", "rejected", before);

					return;
				}

				const plan = prepared.success;
				const attempted = yield* capability.mutation.Execute(plan).pipe(Effect.result);
				const after = yield* observer.Observe(execution.approval.workspace_id).pipe(
					Effect.catch(() =>
						Effect.gen(function* () {
							yield* repository.MarkUnknown(approval_id);

							return yield* Effect.fail(
								new WorkspaceGitObservationError({ reason: "git_failed" }),
							);
						}),
					),
				);

				if (Result.isFailure(attempted)) {
					yield* SettleObservedFailure(
						execution,
						"failure",
						observation_matches_source(after, execution) ? "rejected" : "unknown",
						after,
					);

					return;
				}

				const reconciled = yield* capability.mutation
					.Reconcile(plan, attempted.success)
					.pipe(Effect.result);

				if (
					Result.isFailure(reconciled) ||
					reconciled.success.type !== "applied" ||
					reconciled.success.branch !== execution.approval.target_branch ||
					reconciled.success.head !== execution.target_head
				) {
					yield* SettleObservedFailure(
						execution,
						"failure",
						!Result.isFailure(reconciled) &&
							(reconciled.success.type === "rejected" ||
								reconciled.success.type === "source") &&
							observation_matches_source(after, execution)
							? "rejected"
							: "unknown",
						after,
					);

					return;
				}

				const projected = yield* Project(execution, "post", after).pipe(Effect.result);

				if (Result.isFailure(projected)) {
					yield* repository.MarkUnknown(approval_id);

					return yield* Effect.fail(projected.failure);
				}

				yield* observation_matches_target(after, execution)
					? repository.MarkApplied(approval_id)
					: repository.MarkUnknown(approval_id);
			}).pipe(Effect.tapError(() => repository.MarkUnknown(approval_id).pipe(Effect.ignore)));
		const RecoverExecuting = Effect.gen(function* () {
			const executing = yield* repository.ListExecuting;

			yield* Effect.forEach(
				executing,
				(approval_id) =>
					Effect.gen(function* () {
						const execution = yield* repository.ReadExecution(approval_id);

						yield* Project(execution, "recovery").pipe(Effect.result);
						yield* repository.MarkUnknown(approval_id);
					}),
				{ discard: true },
			);
		});
		const DispatchPending = Effect.gen(function* () {
			const approved = yield* repository.ListApproved;
			const results = yield* Effect.forEach(approved, (approval_id) =>
				Effect.gen(function* () {
					const execution = yield* repository.ReadExecution(approval_id);

					yield* dispatch_fence.Run(
						execution.approval.thread_id,
						ExecuteApproved(approval_id),
					);
				}).pipe(Effect.exit),
			);

			return results.some(Exit.isFailure);
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
		const Request = (input: WorkspaceGitCheckoutRequestInput) =>
			Schema.decodeUnknownEffect(CheckoutRequestInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(
					() => new WorkspaceGitCheckoutFailure({ reason: "invalid_request" }),
				),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const existing = yield* repository.ReadBySourceCommand(decoded.message_id);

						if (Option.isSome(existing)) {
							if (!request_matches_existing(decoded, existing.value)) {
								return yield* new WorkspaceGitCheckoutConflict({
									reason: "request_conflict",
								});
							}

							return { ...existing.value, status: "duplicate" as const };
						}

						const session = (yield* sessions.Query({
							workspace_id: decoded.workspace_id,
						})).session;

						if (
							session === undefined ||
							session.state !== "ready" ||
							session.has_diff ||
							session.version !== decoded.expected_session_version
						) {
							return yield* new WorkspaceGitCheckoutConflict({
								reason: "session_stale",
							});
						}

						if (session.branch === decoded.target_branch) {
							return yield* new WorkspaceGitCheckoutFailure({ reason: "no_change" });
						}

						const capability = yield* registry
							.Get(decoded.workspace_id)
							.pipe(
								Effect.mapError(
									() => new WorkspaceGitCheckoutFailure({ reason: "git_failed" }),
								),
							);
						const target_head = yield* capability.read
							.ResolveLocalBranch(decoded.target_branch)
							.pipe(
								Effect.mapError(
									() => new WorkspaceGitCheckoutFailure({ reason: "git_failed" }),
								),
							);

						if (Option.isNone(target_head)) {
							return yield* new WorkspaceGitCheckoutFailure({
								reason: "target_missing",
							});
						}

						const request_fingerprint = yield* Fingerprint(decoded);

						return yield* repository.Request({
							approval_id: `workspace_git_checkout:${decoded.message_id}`,
							expected_session_version: decoded.expected_session_version,
							request_fingerprint,
							source_command: {
								message_id: decoded.message_id,
								sent_at: decoded.sent_at,
							},
							target_branch: decoded.target_branch,
							target_head: target_head.value,
							thread_id: decoded.thread_id,
							workspace_id: decoded.workspace_id,
						});
					}),
				),
			);
		const Respond = (input: WorkspaceGitCheckoutDecisionInput) =>
			Schema.decodeUnknownEffect(CheckoutDecisionInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(
					() => new WorkspaceGitCheckoutFailure({ reason: "invalid_request" }),
				),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const acceptance = yield* repository.Decide({
							approval_id: decoded.approval_id,
							approved: decoded.approved,
							decision_command: {
								message_id: decoded.message_id,
								sent_at: decoded.sent_at,
							},
							thread_id: decoded.thread_id,
						});

						yield* WakeDispatcher;

						return acceptance;
					}),
				),
			);
		const Recover = Effect.gen(function* () {
			yield* RecoverExecuting;
			yield* WakeDispatcher;
		});
		const QuiesceThread = (thread_id: string) => dispatch_fence.Quiesce(thread_id, Effect.void);

		yield* Recover;

		return { QuiesceThread, Recover, Request, Respond };
	}),
);

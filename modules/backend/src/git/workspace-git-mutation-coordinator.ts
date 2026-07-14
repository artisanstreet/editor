import {
	Context,
	Crypto,
	Data,
	Effect,
	Encoding,
	Exit,
	Layer,
	Match,
	Option,
	Ref,
	Result,
	Schema,
	Scope,
} from "effect";

import {
	Identifier,
	IsoDateTime,
	type WorkspaceGitSession,
	WorkspaceGitMutationApprovalResponseRequest,
	WorkspaceGitMutationContinuationOperation,
	WorkspaceGitMutationOperation,
	WorkspaceGitMutationRequest,
} from "@artisan/protocol";

import type { GitMutationReconciliation } from "./git-mutation";
import {
	WorkspaceGitMutationConflict,
	WorkspaceGitMutationRepository,
	type WorkspaceGitMutationAcceptance,
	type WorkspaceGitMutationExecution,
	type WorkspaceGitMutationRepositoryError,
	type WorkspaceGitMutationSettlement,
} from "./workspace-git-mutation-repository";
import {
	WorkspaceGitSessionService,
	type WorkspaceGitSessionServiceError,
} from "./workspace-git-session-service";
import { WorkspaceGitRegistry } from "./workspace-git-registry";
import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";

const MutationRequestInput = Schema.Struct({
	action_approval_id: Schema.optional(Identifier),
	expected_session_version: Schema.Int.check(Schema.isGreaterThan(0)),
	message_id: Identifier,
	operation: WorkspaceGitMutationOperation,
	sent_at: IsoDateTime,
	thread_id: Identifier,
	workspace_id: Identifier,
});

const MutationDecisionInput = Schema.Struct({
	...WorkspaceGitMutationApprovalResponseRequest.fields,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

/** Supplies one Git mutation approval request with durable command metadata. */
export type WorkspaceGitMutationRequestInput = typeof MutationRequestInput.Type;

/** Supplies one Git mutation approval response with durable command metadata. */
export type WorkspaceGitMutationDecisionInput = typeof MutationDecisionInput.Type;

/** Reports malformed requests or unavailable native Git planning without private output. */
export class WorkspaceGitMutationCoordinatorFailure extends Data.TaggedError(
	"WorkspaceGitMutationCoordinatorFailure",
)<{ readonly reason: "git_failed" | "invalid_request" }> {}

/** Represents synchronous failures surfaced by the generic Git mutation coordinator. */
export type WorkspaceGitMutationCoordinatorError =
	| WorkspaceGitMutationCoordinatorFailure
	| WorkspaceGitMutationRepositoryError
	| WorkspaceGitSessionServiceError;

/** Coordinates approval, exactly-once execution, reconciliation, and durable projection. */
export class WorkspaceGitMutationCoordinator extends Context.Service<
	WorkspaceGitMutationCoordinator,
	{
		readonly QuiesceThread: (thread_id: string) => Effect.Effect<void>;
		readonly Recover: Effect.Effect<void, WorkspaceGitMutationCoordinatorError>;
		readonly Request: (
			input: WorkspaceGitMutationRequestInput,
		) => Effect.Effect<WorkspaceGitMutationAcceptance, WorkspaceGitMutationCoordinatorError>;
		readonly Respond: (
			input: WorkspaceGitMutationDecisionInput,
		) => Effect.Effect<WorkspaceGitMutationAcceptance, WorkspaceGitMutationCoordinatorError>;
	}
>()("Artisan/WorkspaceGitMutationCoordinator") {}

type DispatchState = "idle" | "pending" | "running";

function mutation_fingerprint(input: WorkspaceGitMutationRequestInput) {
	return JSON.stringify({
		...(input.action_approval_id === undefined
			? {}
			: { action_approval_id: input.action_approval_id }),
		expected_session_version: input.expected_session_version,
		message_id: input.message_id,
		operation: input.operation,
		sent_at: input.sent_at,
		thread_id: input.thread_id,
		workspace_id: input.workspace_id,
	});
}

function settlement_from(
	execution: WorkspaceGitMutationExecution,
	reconciliation: GitMutationReconciliation,
): WorkspaceGitMutationSettlement {
	const identity = {
		approval_id: execution.approval.approval_id,
		claim_token: execution.claim_token,
	};

	if (reconciliation.type === "applied") {
		return {
			...identity,
			...(reconciliation.branch === undefined ? {} : { branch: reconciliation.branch }),
			head: reconciliation.head,
			...(reconciliation.remote_head === undefined
				? {}
				: { remote_head: reconciliation.remote_head }),
			type: "applied",
		};
	}

	if (reconciliation.type === "action_required") {
		return { ...identity, action: reconciliation.action, type: "action_required" };
	}

	if (reconciliation.type === "rejected") {
		return { ...identity, reason: reconciliation.reason, type: "rejected" };
	}

	return {
		...identity,
		reason:
			reconciliation.type === "source"
				? "interrupted"
				: execution.operation.type === "push"
					? "remote_unverifiable"
					: "verification_failed",
		type: "outcome_unknown",
	};
}

function reconciliation_matches_projection(
	reconciliation: GitMutationReconciliation,
	session: WorkspaceGitSession,
) {
	return (
		reconciliation.type !== "applied" ||
		(reconciliation.branch === session.branch && reconciliation.head === session.head)
	);
}

/** Supplies the scoped dispatcher that never retries a persisted Git execution. */
export const WorkspaceGitMutationCoordinatorLive = Layer.effect(
	WorkspaceGitMutationCoordinator,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const registry = yield* WorkspaceGitRegistry;
		const repository = yield* WorkspaceGitMutationRepository;
		const sessions = yield* WorkspaceGitSessionService;
		const dispatch_fence = yield* MakeThreadDispatchFence;
		const dispatch_state = yield* Ref.make<DispatchState>("idle");
		const service_scope = yield* Scope.make();

		yield* Effect.addFinalizer(() =>
			Scope.close(service_scope, Exit.succeed(undefined)).pipe(
				Effect.andThen(repository.AbandonOwnedExecutions),
				Effect.ignore,
			),
		);

		const Fingerprint = (input: WorkspaceGitMutationRequestInput) =>
			crypto.digest("SHA-256", new TextEncoder().encode(mutation_fingerprint(input))).pipe(
				Effect.map(Encoding.encodeHex),
				Effect.mapError(
					() => new WorkspaceGitMutationCoordinatorFailure({ reason: "invalid_request" }),
				),
			);
		const GetCapability = (workspace_id: string) =>
			registry
				.Get(workspace_id)
				.pipe(
					Effect.mapError(
						() => new WorkspaceGitMutationCoordinatorFailure({ reason: "git_failed" }),
					),
				);
		const Project = (execution: WorkspaceGitMutationExecution) =>
			sessions.Project({
				kind: "mutation",
				operation_id: `workspace_git_mutation:${execution.approval.approval_id}:projection`,
				sent_at: execution.approval.updated_at,
				thread_id: execution.approval.thread_id,
				workspace_id: execution.approval.workspace_id,
			});
		const Heartbeat = (execution: WorkspaceGitMutationExecution) =>
			Effect.forever(
				Effect.sleep("5 seconds").pipe(
					Effect.andThen(
						repository.RenewLease({
							approval_id: execution.approval.approval_id,
							claim_token: execution.claim_token,
						}),
					),
				),
			);
		const WithLease = <A, E, R>(
			execution: WorkspaceGitMutationExecution,
			effect: Effect.Effect<A, E, R>,
		) => Effect.raceFirst(effect, Heartbeat(execution));
		const ContinueExecution = (execution: WorkspaceGitMutationExecution) =>
			Effect.gen(function* () {
				const capability = yield* GetCapability(execution.approval.workspace_id);
				const identity = {
					approval_id: execution.approval.approval_id,
					claim_token: execution.claim_token,
				};
				const projection = yield* Project(execution);
				const observed_reconciliation =
					execution.reconciliation ??
					(yield* capability.mutation.Reconcile(execution.plan, execution.attempt).pipe(
						Effect.mapError(
							() =>
								new WorkspaceGitMutationCoordinatorFailure({
									reason: "git_failed",
								}),
						),
					));

				if (
					execution.reconciliation !== undefined &&
					!reconciliation_matches_projection(execution.reconciliation, projection.session)
				) {
					return yield* new WorkspaceGitMutationCoordinatorFailure({
						reason: "git_failed",
					});
				}

				const reconciliation = reconciliation_matches_projection(
					observed_reconciliation,
					projection.session,
				)
					? observed_reconciliation
					: ({ type: "outcome_unknown" } as const);

				if (execution.reconciliation === undefined) {
					yield* repository.RecordReconciliation(identity, reconciliation);
				}

				return yield* repository.Settle(settlement_from(execution, reconciliation));
			});
		const ExecuteApproved = (approval_id: string) =>
			Effect.gen(function* () {
				const execution_claim = yield* repository
					.MarkExecuting(approval_id)
					.pipe(Effect.result);

				if (Result.isFailure(execution_claim)) {
					if (
						execution_claim.failure instanceof WorkspaceGitMutationConflict &&
						execution_claim.failure.reason === "session_stale"
					) {
						yield* repository.RejectApproved({
							approval_id,
							reason: "stale_session",
						});

						return;
					}

					return yield* Effect.fail(execution_claim.failure);
				}

				if (execution_claim.success.status === "duplicate") {
					return;
				}

				const execution = yield* repository.ReadExecution(approval_id);
				const capability = yield* GetCapability(execution.approval.workspace_id);
				const identity = {
					approval_id,
					claim_token: execution.claim_token,
				};

				return yield* WithLease(
					execution,
					Effect.gen(function* () {
						const attempted = yield* repository.ExecuteClaimed(
							identity,
							capability.mutation.Execute(execution.plan).pipe(Effect.result),
						);

						if (Result.isFailure(attempted)) {
							return yield* ContinueExecution(execution);
						}

						yield* repository.RecordAttempt(identity, attempted.success);

						return yield* ContinueExecution({
							...execution,
							attempt: attempted.success,
						});
					}),
				);
			});
		const RecoverExecuting = Effect.gen(function* () {
			const executing = yield* repository.ListExecuting;
			const resumable = executing.filter((dispatch) => dispatch.recovery !== "waiting");
			const results = yield* Effect.forEach(resumable, (dispatch) =>
				dispatch_fence
					.Run(
						dispatch.thread_id,
						Match.value(dispatch.recovery).pipe(
							Match.when("owned", () =>
								Effect.flatMap(
									repository.ReadExecution(dispatch.approval_id),
									(execution) =>
										WithLease(execution, ContinueExecution(execution)),
								),
							),
							Match.when("quarantine", () =>
								repository
									.QuarantineInterrupted(dispatch.approval_id)
									.pipe(Effect.asVoid),
							),
							Match.when("recoverable", () =>
								Effect.flatMap(
									repository.ClaimRecovery(dispatch.approval_id),
									(claimed) =>
										Option.match(claimed, {
											onNone: () => Effect.void,
											onSome: (execution) =>
												WithLease(execution, ContinueExecution(execution)),
										}),
								),
							),
							Match.orElse(() => Effect.void),
						),
					)
					.pipe(Effect.exit),
			);

			return results.some(Exit.isFailure) || resumable.length !== executing.length;
		});
		const DispatchApproved = Effect.gen(function* () {
			const approved = yield* repository.ListApproved;
			const results = yield* Effect.forEach(approved, (dispatch) =>
				dispatch_fence
					.Run(dispatch.thread_id, ExecuteApproved(dispatch.approval_id))
					.pipe(Effect.exit),
			);

			return results.some(Exit.isFailure);
		});
		const DispatchWork = Effect.gen(function* () {
			const recover_retry = yield* RecoverExecuting;
			const approved_retry = yield* DispatchApproved;

			return recover_retry || approved_retry;
		});
		const DispatchLoop = Effect.gen(function* () {
			while (true) {
				const result = yield* DispatchWork.pipe(Effect.exit);
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
		const DecodeRequest = (input: WorkspaceGitMutationRequestInput) =>
			Schema.decodeUnknownEffect(MutationRequestInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.flatMap((decoded) =>
					Schema.decodeUnknownEffect(WorkspaceGitMutationRequest, {
						onExcessProperty: "error",
					})({
						...(decoded.action_approval_id === undefined
							? {}
							: { action_approval_id: decoded.action_approval_id }),
						expected_session_version: decoded.expected_session_version,
						operation: decoded.operation,
						workspace_id: decoded.workspace_id,
					}).pipe(Effect.as(decoded)),
				),
				Effect.mapError(
					() => new WorkspaceGitMutationCoordinatorFailure({ reason: "invalid_request" }),
				),
			);
		const Request = (input: WorkspaceGitMutationRequestInput) =>
			DecodeRequest(input).pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const request_fingerprint = yield* Fingerprint(decoded);
						const replay_input = {
							...(decoded.action_approval_id === undefined
								? {}
								: { action_approval_id: decoded.action_approval_id }),
							approval_id: `workspace_git_mutation:${decoded.message_id}`,
							expected_session_version: decoded.expected_session_version,
							operation: decoded.operation,
							request_fingerprint,
							source_command: {
								message_id: decoded.message_id,
								sent_at: decoded.sent_at,
							},
							thread_id: decoded.thread_id,
							workspace_id: decoded.workspace_id,
						};
						const replay = yield* repository.ReplayRequest(replay_input);

						if (Option.isSome(replay)) {
							return replay.value;
						}

						const session = (yield* sessions.Query({
							workspace_id: decoded.workspace_id,
						})).session;

						if (
							session === undefined ||
							session.state === "unavailable" ||
							session.version !== decoded.expected_session_version
						) {
							return yield* new WorkspaceGitMutationConflict({
								reason: "session_stale",
							});
						}

						const capability = yield* GetCapability(decoded.workspace_id);
						const preparation =
							decoded.action_approval_id === undefined
								? decoded.operation
								: {
										action_anchor: yield* repository.ReadActionAnchor({
											action_approval_id: decoded.action_approval_id,
											operation: yield* Schema.decodeUnknownEffect(
												WorkspaceGitMutationContinuationOperation,
												{ onExcessProperty: "error" },
											)(decoded.operation).pipe(
												Effect.mapError(
													() =>
														new WorkspaceGitMutationConflict({
															reason: "action_conflict",
														}),
												),
											),
											thread_id: decoded.thread_id,
											workspace_id: decoded.workspace_id,
										}),
										operation: decoded.operation,
									};
						const plan = yield* capability.mutation.Prepare(preparation).pipe(
							Effect.mapError(
								() =>
									new WorkspaceGitMutationCoordinatorFailure({
										reason: "git_failed",
									}),
							),
						);

						return yield* repository.Request({ ...replay_input, plan });
					}),
				),
			);
		const Respond = (input: WorkspaceGitMutationDecisionInput) =>
			Schema.decodeUnknownEffect(MutationDecisionInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(
					() => new WorkspaceGitMutationCoordinatorFailure({ reason: "invalid_request" }),
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

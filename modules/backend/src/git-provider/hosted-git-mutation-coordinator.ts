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
	Result,
	Schema,
	Scope,
	Stream,
	SubscriptionRef,
} from "effect";

import {
	HostedGitMutationApprovalResponseRequest,
	HostedGitMutationCommandRequest,
	Identifier,
	IsoDateTime,
} from "@artisan/protocol";

import { GitProviderError } from "./git-provider";
import { GitProviderRegistry } from "./git-provider-registry";
import {
	HostedGitMutationRepository,
	type HostedGitMutationAcceptance,
	type HostedGitMutationExecution,
	type HostedGitMutationRepositoryError,
} from "./hosted-git-mutation-repository";
import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";

const MutationRequestInput = Schema.Struct({
	...HostedGitMutationCommandRequest.fields,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

const MutationDecisionInput = Schema.Struct({
	...HostedGitMutationApprovalResponseRequest.fields,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

/** Supplies one hosted Git mutation request with durable source metadata. */
export type HostedGitMutationRequestInput = typeof MutationRequestInput.Type;

/** Supplies one hosted Git mutation decision with durable source metadata. */
export type HostedGitMutationDecisionInput = typeof MutationDecisionInput.Type;

/** Reports source-safe malformed input at the hosted Git mutation boundary. */
export class HostedGitMutationCoordinatorFailure extends Data.TaggedError(
	"HostedGitMutationCoordinatorFailure",
)<{ readonly reason: "invalid_request" }> {}

/** Represents synchronous hosted Git mutation coordination failures. */
export type HostedGitMutationCoordinatorError =
	| HostedGitMutationCoordinatorFailure
	| HostedGitMutationRepositoryError;

/** Coordinates hosted mutation approval, durable provider execution, and recovery. */
export class HostedGitMutationCoordinator extends Context.Service<
	HostedGitMutationCoordinator,
	{
		readonly AwaitIdle: Effect.Effect<void>;
		readonly QuiesceThread: (thread_id: string) => Effect.Effect<void>;
		readonly Recover: Effect.Effect<void, HostedGitMutationCoordinatorError>;
		readonly Request: (
			input: HostedGitMutationRequestInput,
		) => Effect.Effect<HostedGitMutationAcceptance, HostedGitMutationCoordinatorError>;
		readonly Respond: (
			input: HostedGitMutationDecisionInput,
		) => Effect.Effect<HostedGitMutationAcceptance, HostedGitMutationCoordinatorError>;
	}
>()("Artisan/HostedGitMutationCoordinator") {}

type DispatchState = "idle" | "pending" | "running";

function coordinator_failure() {
	return new HostedGitMutationCoordinatorFailure({ reason: "invalid_request" });
}

function mutation_fingerprint(input: HostedGitMutationRequestInput) {
	return JSON.stringify({
		message_id: input.message_id,
		mutation: input.mutation,
		selection: input.selection,
		sent_at: input.sent_at,
		thread_id: input.thread_id,
	});
}

function approval_id(message_id: string) {
	return `hosted_git_mutation:${message_id}`;
}

function provider_settlement(execution: HostedGitMutationExecution, error: GitProviderError) {
	const identity = {
		approval_id: execution.approval.approval_id,
		claim_token: execution.claim_token,
	};

	return Match.value(error.reason).pipe(
		Match.when("outcome_unknown", () => ({
			...identity,
			reason: "provider_outcome_unknown" as const,
			type: "outcome_unknown" as const,
		})),
		Match.whenOr("account_not_active", "auth_required", () => ({
			...identity,
			reason: "authentication_required" as const,
			type: "rejected" as const,
		})),
		Match.when("permission_denied", () => ({
			...identity,
			reason: "permission_denied" as const,
			type: "rejected" as const,
		})),
		Match.when("rate_limited", () => ({
			...identity,
			reason: "rate_limited" as const,
			type: "rejected" as const,
		})),
		Match.when("remote_rejected", () => ({
			...identity,
			reason: "remote_rejected" as const,
			type: "rejected" as const,
		})),
		Match.whenOr("not_found", "stale_repository", () => ({
			...identity,
			reason: "snapshot_stale" as const,
			type: "rejected" as const,
		})),
		Match.whenOr("invalid_cursor", "invalid_input", "invalid_response", () => ({
			...identity,
			reason: "invalid_provider_response" as const,
			type: "rejected" as const,
		})),
		Match.orElse(() => ({
			...identity,
			reason: "provider_unavailable" as const,
			type: "rejected" as const,
		})),
	);
}

/** Supplies the scoped hosted mutation dispatcher and non-replay recovery policy. */
export const HostedGitMutationCoordinatorLive = Layer.effect(
	HostedGitMutationCoordinator,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const providers = yield* GitProviderRegistry;
		const repository = yield* HostedGitMutationRepository;
		const dispatch_fence = yield* MakeThreadDispatchFence;
		const dispatch_state = yield* SubscriptionRef.make<DispatchState>("idle");
		const service_scope = yield* Scope.make();

		yield* Effect.addFinalizer(() =>
			Scope.close(service_scope, Exit.succeed(undefined)).pipe(
				Effect.andThen(repository.AbandonOwnedExecutions),
				Effect.ignore,
			),
		);

		const Fingerprint = (input: HostedGitMutationRequestInput) =>
			crypto
				.digest("SHA-256", new TextEncoder().encode(mutation_fingerprint(input)))
				.pipe(Effect.map(Encoding.encodeHex), Effect.mapError(coordinator_failure));
		const ClientMutationId = (input: HostedGitMutationExecution) =>
			crypto.digest("SHA-256", new TextEncoder().encode(input.approval.approval_id)).pipe(
				Effect.map(Encoding.encodeHex),
				Effect.map((digest) => `hosted:${digest}`),
				Effect.mapError(coordinator_failure),
			);
		const Heartbeat = (execution: HostedGitMutationExecution) =>
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
			execution: HostedGitMutationExecution,
			effect: Effect.Effect<A, E, R>,
		) => Effect.raceFirst(effect, Heartbeat(execution));
		const SettleUnknown = (execution: HostedGitMutationExecution) =>
			repository.Settle({
				approval_id: execution.approval.approval_id,
				claim_token: execution.claim_token,
				reason: "provider_outcome_unknown",
				type: "outcome_unknown",
			});
		const ContinueExecution: (
			execution: HostedGitMutationExecution,
		) => Effect.Effect<HostedGitMutationAcceptance, HostedGitMutationCoordinatorError> = (
			execution,
		) =>
			Effect.gen(function* () {
				const identity = {
					approval_id: execution.approval.approval_id,
					claim_token: execution.claim_token,
				};

				if (execution.provider_result !== undefined) {
					return yield* repository.Settle({ ...identity, type: "applied" });
				}

				if (execution.provider_execution_started) {
					return yield* SettleUnknown(execution);
				}

				const provider = yield* providers
					.Get(execution.command.selection.provider_id)
					.pipe(Effect.result);

				if (Result.isFailure(provider)) {
					return yield* repository.Settle({
						...identity,
						reason: "provider_unavailable",
						type: "rejected",
					});
				}

				if (provider.success.ExecuteMutation === undefined) {
					return yield* repository.Settle({
						...identity,
						reason: "unsupported_operation",
						type: "rejected",
					});
				}

				const client_mutation_id = yield* ClientMutationId(execution);
				const attempted = yield* repository
					.ExecuteClaimed(
						identity,
						provider.success
							.ExecuteMutation({
								client_mutation_id,
								mutation: execution.command.mutation,
								selection: execution.command.selection,
							})
							.pipe(
								Effect.tap((result) =>
									repository.RecordProviderResult({ ...identity, result }),
								),
								Effect.result,
							),
					)
					.pipe(Effect.result);

				if (Result.isFailure(attempted)) {
					const persisted = yield* repository
						.ReadExecution(execution.approval.approval_id)
						.pipe(Effect.result);

					if (Result.isSuccess(persisted)) {
						if (persisted.success.provider_result !== undefined) {
							return yield* repository.Settle({ ...identity, type: "applied" });
						}

						if (persisted.success.provider_execution_started) {
							return yield* SettleUnknown(persisted.success);
						}
					}

					return yield* Effect.fail(attempted.failure);
				}

				if (Result.isFailure(attempted.success)) {
					const failure = attempted.success.failure;

					if (failure instanceof GitProviderError) {
						return yield* repository.Settle(provider_settlement(execution, failure));
					}

					const persisted = yield* repository
						.ReadExecution(execution.approval.approval_id)
						.pipe(Effect.result);

					if (
						Result.isSuccess(persisted) &&
						persisted.success.provider_result !== undefined
					) {
						return yield* repository.Settle({ ...identity, type: "applied" });
					}

					return yield* SettleUnknown(execution).pipe(
						Effect.catch(() => Effect.fail(failure)),
					);
				}

				return yield* repository.Settle({ ...identity, type: "applied" });
			});
		const ExecuteApproved = (requested_approval_id: string) =>
			Effect.gen(function* () {
				const execution_claim = yield* repository
					.MarkExecuting(requested_approval_id)
					.pipe(Effect.result);

				if (Result.isFailure(execution_claim)) {
					return yield* Effect.fail(execution_claim.failure);
				}

				if (execution_claim.success.status === "duplicate") {
					return;
				}

				const execution = yield* repository.ReadExecution(requested_approval_id);

				return yield* WithLease(execution, ContinueExecution(execution));
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
				const continue_dispatch = yield* SubscriptionRef.modify(dispatch_state, (state) => {
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
			const start = yield* SubscriptionRef.modify(dispatch_state, (state) =>
				state === "idle" ? ([true, "running"] as const) : ([false, "pending"] as const),
			);

			if (start) {
				yield* Effect.forkIn(DispatchLoop, service_scope);
			}
		});
		const AwaitIdle = SubscriptionRef.changes(dispatch_state).pipe(
			Stream.filter((state) => state === "idle"),
			Stream.runHead,
			Effect.asVoid,
		);
		const Request = (input: HostedGitMutationRequestInput) =>
			Schema.decodeUnknownEffect(MutationRequestInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(coordinator_failure),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const request_fingerprint = yield* Fingerprint(decoded);
						const request = {
							approval_id: approval_id(decoded.message_id),
							command: { mutation: decoded.mutation, selection: decoded.selection },
							request_fingerprint,
							source_command: {
								message_id: decoded.message_id,
								sent_at: decoded.sent_at,
							},
							thread_id: decoded.thread_id,
						};
						const replay = yield* repository.ReplayRequest(request);

						return yield* Option.match(replay, {
							onNone: () => repository.Request(request),
							onSome: Effect.succeed,
						});
					}),
				),
			);
		const Respond = (input: HostedGitMutationDecisionInput) =>
			Schema.decodeUnknownEffect(MutationDecisionInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(coordinator_failure),
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
		const Recover = WakeDispatcher;
		const QuiesceThread = (thread_id: string) => dispatch_fence.Quiesce(thread_id, Effect.void);

		yield* Recover;

		return { AwaitIdle, QuiesceThread, Recover, Request, Respond };
	}),
);

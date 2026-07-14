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
	HostedProjectCloneApprovalResponseRequest,
	HostedProjectCloneRequest,
	Identifier,
	IsoDateTime,
} from "@artisan/protocol";

import {
	GitProviderCloneRequest,
	GitProviderError,
	type GitProviderCloneRequest as GitProviderCloneRequestValue,
	type GitProviderCloneResult as GitProviderCloneResultValue,
} from "../git-provider/git-provider";
import { GitProviderRegistry } from "../git-provider/git-provider-registry";
import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";
import {
	ThreadProjectAffinityNotFound,
	ThreadProjectAffinityRepository,
	ThreadProjectInitialAttachmentConflict,
} from "../threads/thread-project-affinity-repository";
import { HostedProjectCloneDestination } from "./hosted-project-clone-destination";
import {
	HostedProjectCloneConflict,
	HostedProjectCloneRepository,
	type HostedProjectCloneAcceptance,
	type HostedProjectCloneExecution,
	type HostedProjectCloneRepositoryError,
} from "./hosted-project-clone-repository";
import { ProjectRepository } from "./project-repository";
import type { RegisteredProject } from "./project";

const CloneRequestInput = Schema.Struct({
	destination_path: HostedProjectCloneRequest.fields.destination_path,
	message_id: Identifier,
	repository: HostedProjectCloneRequest.fields.repository,
	selection: HostedProjectCloneRequest.fields.selection,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

const CloneDecisionInput = Schema.Struct({
	...HostedProjectCloneApprovalResponseRequest.fields,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

/** Supplies one hosted clone request with durable command metadata. */
export type HostedProjectCloneRequestInput = typeof CloneRequestInput.Type;

/** Supplies one hosted clone decision with durable command metadata. */
export type HostedProjectCloneDecisionInput = typeof CloneDecisionInput.Type;

/** Reports a source-safe failure at a hosted clone coordination boundary. */
export class HostedProjectCloneCoordinatorFailure extends Data.TaggedError(
	"HostedProjectCloneCoordinatorFailure",
)<{
	readonly reason:
		| "destination_unavailable"
		| "invalid_request"
		| "provider_unavailable"
		| "repository_unavailable"
		| "thread_unavailable";
}> {}

/** Represents synchronous failures surfaced by the hosted clone coordinator. */
export type HostedProjectCloneCoordinatorError =
	| HostedProjectCloneCoordinatorFailure
	| HostedProjectCloneRepositoryError;

/** Coordinates clone reuse, approval, exactly-once execution, registration, and attachment. */
export class HostedProjectCloneCoordinator extends Context.Service<
	HostedProjectCloneCoordinator,
	{
		readonly AwaitIdle: Effect.Effect<void>;
		readonly QuiesceThread: (thread_id: string) => Effect.Effect<void>;
		readonly Recover: Effect.Effect<void, HostedProjectCloneCoordinatorError>;
		readonly Request: (
			input: HostedProjectCloneRequestInput,
		) => Effect.Effect<HostedProjectCloneAcceptance, HostedProjectCloneCoordinatorError>;
		readonly Respond: (
			input: HostedProjectCloneDecisionInput,
		) => Effect.Effect<HostedProjectCloneAcceptance, HostedProjectCloneCoordinatorError>;
	}
>()("Artisan/HostedProjectCloneCoordinator") {}

type DispatchState = "idle" | "pending" | "running";
type Attachment = "attached" | "already_attached";

function coordinator_failure(reason: HostedProjectCloneCoordinatorFailure["reason"]) {
	return new HostedProjectCloneCoordinatorFailure({ reason });
}

function clone_fingerprint(input: HostedProjectCloneRequestInput) {
	return JSON.stringify({
		destination_path: input.destination_path,
		message_id: input.message_id,
		repository: input.repository,
		selection: input.selection,
		sent_at: input.sent_at,
		thread_id: input.thread_id,
	});
}

function approval_id(message_id: string) {
	return `hosted_project_clone:${message_id}`;
}

function attachment_id(message_id: string) {
	return `hosted_project_clone_attachment:${message_id}`;
}

function hosted_identity(request: GitProviderCloneRequestValue) {
	return {
		canonical_host: request.repository.identity.host,
		native_id: request.repository.origin.native_id,
		provider_id: request.repository.identity.provider_id,
	};
}

function registration_from(
	execution: HostedProjectCloneExecution,
	result: GitProviderCloneResultValue,
) {
	const repository = result.repository;

	return {
		canonical_root: result.canonical_root,
		display_name: repository.identity.name,
		hosted_origin: {
			canonical_host: repository.identity.host,
			clone_url: repository.clone_url,
			fetch_url: repository.clone_url,
			name: repository.identity.name,
			native_id: repository.origin.native_id,
			owner: repository.identity.owner,
			provider_id: repository.identity.provider_id,
			push_url: repository.clone_url,
			remote_name: "origin" as const,
			selected_account_login: execution.preparation.selection.account_login,
			web_url: repository.web_url,
		},
	};
}

function attachment_from(status: "accepted" | "already_attached" | "duplicate"): Attachment {
	return status === "already_attached" ? "already_attached" : "attached";
}

/** Supplies the scoped hosted clone dispatcher and conservative crash recovery policy. */
export const HostedProjectCloneCoordinatorLive = Layer.effect(
	HostedProjectCloneCoordinator,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const destinations = yield* HostedProjectCloneDestination;
		const providers = yield* GitProviderRegistry;
		const projects = yield* ProjectRepository;
		const repository = yield* HostedProjectCloneRepository;
		const thread_projects = yield* ThreadProjectAffinityRepository;
		const dispatch_fence = yield* MakeThreadDispatchFence;
		const dispatch_state = yield* SubscriptionRef.make<DispatchState>("idle");
		const service_scope = yield* Scope.make();

		yield* Effect.addFinalizer(() =>
			Scope.close(service_scope, Exit.succeed(undefined)).pipe(
				Effect.andThen(repository.AbandonOwnedExecutions),
				Effect.ignore,
			),
		);

		const Fingerprint = (input: HostedProjectCloneRequestInput) =>
			crypto.digest("SHA-256", new TextEncoder().encode(clone_fingerprint(input))).pipe(
				Effect.map(Encoding.encodeHex),
				Effect.mapError(() => coordinator_failure("invalid_request")),
			);
		const FindExisting = (request: GitProviderCloneRequestValue) =>
			projects
				.FindByHostedIdentity(hosted_identity(request))
				.pipe(Effect.mapError(() => coordinator_failure("repository_unavailable")));
		const AttachProject = (
			project: RegisteredProject,
			source_event_id: string,
			thread_id: string,
		) =>
			thread_projects.AttachInitialProject({
				attachment_id: attachment_id(source_event_id),
				project_id: project.project.project_id,
				source_event_id,
				thread_id,
			});
		const RecordReused = (
			input: HostedProjectCloneRequestInput,
			request: GitProviderCloneRequestValue,
			request_fingerprint: string,
			project: RegisteredProject,
		) =>
			AttachProject(project, input.message_id, input.thread_id).pipe(
				Effect.mapError(() => coordinator_failure("thread_unavailable")),
				Effect.flatMap((attachment) =>
					repository.RecordReused({
						approval_id: approval_id(input.message_id),
						attachment: attachment_from(attachment.status),
						destination_path: project.project.root_path,
						registered_project: project,
						request,
						request_fingerprint,
						source_command: {
							message_id: input.message_id,
							sent_at: input.sent_at,
						},
						thread_id: input.thread_id,
					}),
				),
			);
		const ReuseIfRegistered = (
			input: HostedProjectCloneRequestInput,
			request: GitProviderCloneRequestValue,
			request_fingerprint: string,
		) =>
			FindExisting(request).pipe(
				Effect.flatMap(
					Option.match({
						onNone: () => Effect.succeed(Option.none<HostedProjectCloneAcceptance>()),
						onSome: (project) =>
							RecordReused(input, request, request_fingerprint, project).pipe(
								Effect.map(Option.some),
							),
					}),
				),
			);
		const DecodeRequest = (input: HostedProjectCloneRequestInput) =>
			Schema.decodeUnknownEffect(CloneRequestInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.flatMap((decoded) =>
					Schema.decodeUnknownEffect(GitProviderCloneRequest, {
						onExcessProperty: "error",
					})({ repository: decoded.repository, selection: decoded.selection }).pipe(
						Effect.map((request) => ({ decoded, request })),
					),
				),
				Effect.mapError(() => coordinator_failure("invalid_request")),
			);

		const SettleRejected = (
			execution: HostedProjectCloneExecution,
			reason: "destination_unavailable" | "provider_unavailable",
		) =>
			repository.Settle({
				approval_id: execution.approval.approval_id,
				claim_token: execution.claim_token,
				reason,
				type: "rejected",
			});
		const SettleUnknown = (execution: HostedProjectCloneExecution) =>
			repository.Settle({
				approval_id: execution.approval.approval_id,
				claim_token: execution.claim_token,
				reason: "verification_failed",
				type: "outcome_unknown",
			});
		const Heartbeat = (execution: HostedProjectCloneExecution) =>
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
			execution: HostedProjectCloneExecution,
			effect: Effect.Effect<A, E, R>,
		) => Effect.raceFirst(effect, Heartbeat(execution));
		const AttachRegistered = (
			execution: HostedProjectCloneExecution,
			project: RegisteredProject,
		) =>
			Effect.gen(function* () {
				const attachment = yield* AttachProject(
					project,
					execution.approval.source_command_id,
					execution.approval.thread_id,
				).pipe(Effect.result);
				const identity = {
					approval_id: execution.approval.approval_id,
					claim_token: execution.claim_token,
				};

				if (Result.isFailure(attachment)) {
					if (
						attachment.failure instanceof ThreadProjectInitialAttachmentConflict ||
						attachment.failure instanceof ThreadProjectAffinityNotFound
					) {
						return yield* repository.Settle({
							...identity,
							project: project.project,
							type: "attachment_conflict",
						});
					}

					return yield* coordinator_failure("repository_unavailable");
				}

				return yield* repository.Settle({
					...identity,
					attachment: attachment_from(attachment.success.status),
					project: project.project,
					type: "applied",
				});
			});
		const RegisterAndAttach = (
			execution: HostedProjectCloneExecution,
			result: GitProviderCloneResultValue,
		) =>
			Effect.gen(function* () {
				const registration = yield* projects
					.RegisterHosted(registration_from(execution, result))
					.pipe(Effect.mapError(() => coordinator_failure("repository_unavailable")));
				const project = registration.project;

				yield* repository.RecordRegisteredProject({
					approval_id: execution.approval.approval_id,
					claim_token: execution.claim_token,
					project,
				});

				return yield* AttachRegistered(execution, project);
			});
		const ContinueExecution: (
			execution: HostedProjectCloneExecution,
		) => Effect.Effect<HostedProjectCloneAcceptance, HostedProjectCloneCoordinatorError> = (
			execution,
		) =>
			Effect.gen(function* () {
				if (execution.registered_project !== undefined) {
					return yield* AttachRegistered(execution, execution.registered_project);
				}

				if (execution.clone_result !== undefined) {
					return yield* RegisterAndAttach(execution, execution.clone_result);
				}

				const destination_ready = yield* destinations
					.WithPinned(execution.destination, () => Effect.void)
					.pipe(Effect.result);

				if (Result.isFailure(destination_ready)) {
					return yield* SettleRejected(execution, "destination_unavailable");
				}

				const provider = yield* providers
					.Get(execution.request.repository.identity.provider_id)
					.pipe(Effect.result);

				if (Result.isFailure(provider)) {
					return yield* SettleRejected(execution, "provider_unavailable");
				}

				const identity = {
					approval_id: execution.approval.approval_id,
					claim_token: execution.claim_token,
				};
				const attempted = yield* repository.ExecuteClaimed(
					identity,
					destinations
						.WithPinned(execution.destination, (destination) =>
							provider.success.Clone({
								destination,
								preparation: execution.preparation,
							}),
						)
						.pipe(
							Effect.tap((result) =>
								repository.RecordCloneResult({ ...identity, result }),
							),
							Effect.result,
						),
				);

				if (Result.isFailure(attempted)) {
					return yield* attempted.failure instanceof GitProviderError &&
					attempted.failure.reason !== "outcome_unknown"
						? SettleRejected(execution, "provider_unavailable")
						: SettleUnknown(execution);
				}

				const persisted = yield* repository.ReadExecution(execution.approval.approval_id);

				return yield* ContinueExecution(persisted);
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
		const Request = (input: HostedProjectCloneRequestInput) =>
			DecodeRequest(input).pipe(
				Effect.flatMap(({ decoded, request }) =>
					Effect.gen(function* () {
						const request_fingerprint = yield* Fingerprint(decoded);
						const replay = yield* repository.ReplayRequest({
							request,
							request_fingerprint,
							source_command: {
								message_id: decoded.message_id,
								sent_at: decoded.sent_at,
							},
							thread_id: decoded.thread_id,
						});

						if (Option.isSome(replay)) {
							return replay.value;
						}

						const initial_reuse = yield* ReuseIfRegistered(
							decoded,
							request,
							request_fingerprint,
						);

						if (Option.isSome(initial_reuse)) {
							return initial_reuse.value;
						}

						const provider = yield* providers
							.Get(request.repository.identity.provider_id)
							.pipe(
								Effect.mapError(() => coordinator_failure("provider_unavailable")),
							);
						const preparation = yield* provider
							.PrepareClone(request)
							.pipe(
								Effect.mapError(() => coordinator_failure("provider_unavailable")),
							);
						const prepared_reuse = yield* ReuseIfRegistered(
							decoded,
							request,
							request_fingerprint,
						);

						if (Option.isSome(prepared_reuse)) {
							return prepared_reuse.value;
						}

						const destination = yield* destinations
							.Plan(decoded.destination_path)
							.pipe(
								Effect.mapError(() =>
									coordinator_failure("destination_unavailable"),
								),
							);
						const planned_reuse = yield* ReuseIfRegistered(
							decoded,
							request,
							request_fingerprint,
						);

						if (Option.isSome(planned_reuse)) {
							return planned_reuse.value;
						}

						return yield* repository
							.Request({
								approval_id: approval_id(decoded.message_id),
								destination,
								preparation,
								request,
								request_fingerprint,
								source_command: {
									message_id: decoded.message_id,
									sent_at: decoded.sent_at,
								},
								thread_id: decoded.thread_id,
							})
							.pipe(
								Effect.catch((error) => {
									if (
										!(error instanceof HostedProjectCloneConflict) ||
										error.reason !== "claim_conflict"
									) {
										return Effect.fail(error);
									}

									return ReuseIfRegistered(
										decoded,
										request,
										request_fingerprint,
									).pipe(
										Effect.flatMap(
											Option.match({
												onNone: () => Effect.fail(error),
												onSome: Effect.succeed,
											}),
										),
									);
								}),
							);
					}),
				),
			);
		const Respond = (input: HostedProjectCloneDecisionInput) =>
			Schema.decodeUnknownEffect(CloneDecisionInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(() => coordinator_failure("invalid_request")),
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

		return { AwaitIdle, QuiesceThread, Recover, Request, Respond };
	}),
);

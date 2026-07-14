import { isSqlError } from "effect/unstable/sql/SqlError";
import {
	Context,
	Crypto,
	Data,
	DateTime,
	Effect,
	Encoding,
	Exit,
	Layer,
	Option,
	Result,
	Schedule,
	Schema,
	Scope,
	Semaphore,
	Stream,
	SubscriptionRef,
} from "effect";

import { Identifier, IsoDateTime, type WorkspaceGitFetchQueryResult } from "@artisan/protocol";

import { GitTransportAuthentication } from "../git-provider/git-transport-authentication";
import { ProjectRepository, type ProjectRepositoryError } from "../projects/project-repository";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";
import { GitFetchError } from "./git-fetch";
import { WorkspaceGitExecutionGate } from "./workspace-git-execution-gate";
import {
	WorkspaceGitFetchConflict,
	WorkspaceGitFetchRepository,
	type WorkspaceGitFetchClaim,
	type WorkspaceGitFetchManualAcceptance,
	type WorkspaceGitFetchPolicyAcceptance,
	type WorkspaceGitFetchRepositoryError,
} from "./workspace-git-fetch-repository";
import { WorkspaceGitNotFoundError, WorkspaceGitRegistry } from "./workspace-git-registry";

const FetchPolicyUpdate = Schema.Struct({
	enabled: Schema.Boolean,
	message_id: Identifier,
	sent_at: IsoDateTime,
});

const ManualFetchRequest = Schema.Struct({
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
	workspace_id: Identifier,
});

export type WorkspaceGitFetchPolicyUpdateInput = typeof FetchPolicyUpdate.Type;
export type WorkspaceGitFetchRequestInput = typeof ManualFetchRequest.Type;

export interface WorkspaceGitFetchCycleResult {
	readonly completed_attempt_ids: ReadonlyArray<string>;
	readonly deferred_attempt_ids: ReadonlyArray<string>;
}

/** Owns the fixed background cadence without exposing it as a user setting. */
export class WorkspaceGitFetchScheduler extends Context.Service<
	WorkspaceGitFetchScheduler,
	{
		readonly Schedule: (task: Effect.Effect<void>) => Effect.Effect<never, never, Scope.Scope>;
	}
>()("Artisan/WorkspaceGitFetchScheduler") {}

export class WorkspaceGitFetchServiceFailure extends Data.TaggedError(
	"WorkspaceGitFetchServiceFailure",
)<{
	readonly cause?: unknown;
	readonly reason: "invalid_request";
}> {}

export type WorkspaceGitFetchServiceError =
	| ProjectRepositoryError
	| WorkspaceGitFetchRepositoryError
	| WorkspaceGitFetchServiceFailure;

/** Owns durable policy, manual requests, leases, and bounded local Git fetch execution. */
export class WorkspaceGitFetchService extends Context.Service<
	WorkspaceGitFetchService,
	{
		readonly AwaitIdle: Effect.Effect<void>;
		readonly Query: Effect.Effect<WorkspaceGitFetchQueryResult, WorkspaceGitFetchServiceError>;
		readonly QuiesceThread: (thread_id: string) => Effect.Effect<void>;
		readonly Request: (
			input: WorkspaceGitFetchRequestInput,
		) => Effect.Effect<WorkspaceGitFetchManualAcceptance, WorkspaceGitFetchServiceError>;
		readonly RunOnce: Effect.Effect<
			WorkspaceGitFetchCycleResult,
			WorkspaceGitFetchServiceError
		>;
		readonly UpdatePolicy: (
			input: WorkspaceGitFetchPolicyUpdateInput,
		) => Effect.Effect<WorkspaceGitFetchPolicyAcceptance, WorkspaceGitFetchServiceError>;
	}
>()("Artisan/WorkspaceGitFetchService") {}

/** Runs automatic checks every five minutes after an explicit startup cycle. */
export const WorkspaceGitFetchSchedulerLive = Layer.succeed(WorkspaceGitFetchScheduler, {
	Schedule: (task) =>
		Effect.repeat(task, Schedule.fixed("5 minutes")).pipe(
			Effect.delay("5 minutes"),
			Effect.andThen(Effect.never),
		),
});

type DispatchState = "idle" | "pending" | "running";

function service_failure(reason: WorkspaceGitFetchServiceFailure["reason"], cause?: unknown) {
	return new WorkspaceGitFetchServiceFailure({
		...(cause === undefined ? {} : { cause }),
		reason,
	});
}

function parse_instant(value: string, label: string) {
	return Effect.fromOption(DateTime.make(value)).pipe(
		Effect.mapError(() => service_failure("invalid_request", new Error(label))),
	);
}

function adjust_instant(
	value: string,
	duration: Parameters<typeof DateTime.add>[1],
	label: string,
) {
	return parse_instant(value, label).pipe(
		Effect.map((instant) => DateTime.formatIso(DateTime.add(instant, duration))),
	);
}

function policy_fingerprint(input: WorkspaceGitFetchPolicyUpdateInput) {
	return JSON.stringify({
		enabled: input.enabled,
		sent_at: input.sent_at,
		type: "workspace.git.fetch.policy.update",
	});
}

function manual_fingerprint(input: WorkspaceGitFetchRequestInput) {
	return JSON.stringify({
		sent_at: input.sent_at,
		thread_id: input.thread_id,
		type: "workspace.git.fetch.request",
		workspace_id: input.workspace_id,
	});
}

/** Supplies one scoped, process-safe dispatcher over the durable fetch repository. */
export const WorkspaceGitFetchServiceLive = Layer.effect(
	WorkspaceGitFetchService,
	Effect.gen(function* () {
		const authentication = yield* GitTransportAuthentication;
		const crypto = yield* Crypto.Crypto;
		const execution_gate = yield* WorkspaceGitExecutionGate;
		const metadata = yield* RuntimeMetadata;
		const projects = yield* ProjectRepository;
		const registry = yield* WorkspaceGitRegistry;
		const repository = yield* WorkspaceGitFetchRepository;
		const scheduler = yield* WorkspaceGitFetchScheduler;
		const cycle_lock = yield* Semaphore.make(1);
		const dispatch_fence = yield* MakeThreadDispatchFence;
		const dispatch_state = yield* SubscriptionRef.make<DispatchState>("idle");
		const service_scope = yield* Scope.make();

		yield* Effect.addFinalizer(() => Scope.close(service_scope, Exit.void));

		const Hash = (value: string) =>
			crypto.digest("SHA-256", new TextEncoder().encode(value)).pipe(
				Effect.map(Encoding.encodeHex),
				Effect.mapError((cause) => service_failure("invalid_request", cause)),
			);
		const ReleaseClaim = (claim: WorkspaceGitFetchClaim) =>
			repository
				.ReleaseClaim({
					attempt_id: claim.attempt_id,
					lease_owner: metadata.instance_id,
					workspace_id: claim.workspace_id,
				})
				.pipe(
					Effect.catch((error) =>
						error instanceof WorkspaceGitFetchConflict
							? Effect.void
							: Effect.fail(error),
					),
				);
		const CompleteClaim = (
			claim: WorkspaceGitFetchClaim,
			attempted_at: string,
			result: "failed" | "succeeded" | "unavailable",
		) =>
			repository.CompleteClaim({
				attempt_id: claim.attempt_id,
				attempted_at,
				lease_owner: metadata.instance_id,
				result,
				workspace_id: claim.workspace_id,
			});
		const FetchClaimed = (claim: WorkspaceGitFetchClaim) =>
			execution_gate
				.Run(
					claim.workspace_id,
					claim.attempt_id,
					Effect.gen(function* () {
						const verified_at = yield* metadata.Now;
						const lease_expires_at = yield* adjust_instant(
							verified_at,
							{ minutes: 4 },
							"Fetch verification lease clock is invalid",
						);
						const verified = yield* repository.VerifyClaim({
							attempt_id: claim.attempt_id,
							lease_expires_at,
							lease_owner: metadata.instance_id,
							now: verified_at,
							workspace_id: claim.workspace_id,
						});

						if (Option.isNone(verified)) {
							yield* ReleaseClaim(claim);

							return "deferred" as const;
						}

						const attempted_at = yield* metadata.Now;
						const project = yield* projects.FindByWorkspaceId({
							workspace_id: claim.workspace_id,
						});

						if (Option.isNone(project)) {
							yield* CompleteClaim(claim, attempted_at, "unavailable");

							return "completed" as const;
						}

						const capability = yield* registry.Get(claim.workspace_id).pipe(
							Effect.map(Option.some),
							Effect.catch((error) =>
								error instanceof WorkspaceGitNotFoundError
									? Effect.succeed(Option.none())
									: Effect.fail(error),
							),
						);

						if (
							Option.isNone(capability) ||
							capability.value.canonical_root !== project.value.project.root_path
						) {
							yield* CompleteClaim(claim, attempted_at, "unavailable");

							return "completed" as const;
						}

						const origin = project.value.hosted_origin;
						const attempted = yield* authentication
							.WithAuthorization<unknown, GitFetchError, never>(
								{
									account_login: origin.selected_account_login,
									host: origin.canonical_host,
									provider_id: origin.provider_id,
									remote_endpoint: origin.fetch_url,
								},
								(authorization) =>
									capability.value.fetch.Fetch(
										{
											remote: origin.remote_name,
											remote_endpoint: origin.fetch_url,
										},
										authorization,
									),
							)
							.pipe(Effect.result);
						const result = Result.isSuccess(attempted)
							? "succeeded"
							: attempted.failure instanceof GitFetchError
								? "failed"
								: "unavailable";

						yield* CompleteClaim(claim, attempted_at, result);

						return "completed" as const;
					}),
				)
				.pipe(
					Effect.catchIf(isSqlError, () =>
						ReleaseClaim(claim).pipe(Effect.as("deferred" as const)),
					),
					Effect.tapError(() => ReleaseClaim(claim).pipe(Effect.ignore)),
				);
		const ClaimManual = (message_id: string) =>
			Effect.gen(function* () {
				const now = yield* metadata.Now;
				const lease_expires_at = yield* adjust_instant(
					now,
					{ minutes: 4 },
					"Manual fetch lease clock is invalid",
				);
				const claim = yield* repository.ClaimManual({
					lease_expires_at,
					lease_owner: metadata.instance_id,
					message_id,
					now,
				});

				return Option.isNone(claim)
					? Option.none<{ readonly attempt_id: string; readonly outcome: "deferred" }>()
					: Option.some({
							attempt_id: claim.value.attempt_id,
							outcome: yield* FetchClaimed(claim.value),
						});
			});
		const ClaimAutomatic = (workspace_id: string) =>
			Effect.gen(function* () {
				const now = yield* metadata.Now;
				const attempt_id = yield* metadata.MakeId("fetch");
				const due_before = yield* adjust_instant(
					now,
					{ minutes: -5 },
					"Automatic fetch clock is invalid",
				);
				const lease_expires_at = yield* adjust_instant(
					now,
					{ minutes: 4 },
					"Automatic fetch lease clock is invalid",
				);
				const claim = yield* repository.ClaimAutomatic({
					attempt_id,
					due_before,
					lease_expires_at,
					lease_owner: metadata.instance_id,
					now,
					workspace_id,
				});

				return Option.isNone(claim)
					? Option.none<{ readonly attempt_id: string; readonly outcome: "deferred" }>()
					: Option.some({
							attempt_id: claim.value.attempt_id,
							outcome: yield* FetchClaimed(claim.value),
						});
			});
		const RunOnceUnlocked = Effect.gen(function* () {
			const pending = yield* repository.ListPendingManual;
			const manual = yield* Effect.forEach(pending, (operation) =>
				dispatch_fence
					.Run(operation.thread_id, ClaimManual(operation.message_id))
					.pipe(Effect.map(Option.flatten)),
			);
			const workspace_ids = yield* registry.ListWorkspaceIds;
			const automatic = yield* Effect.forEach(workspace_ids, ClaimAutomatic);
			const attempts = [...manual, ...automatic]
				.filter(Option.isSome)
				.map(({ value }) => value);
			const deferred_manual_attempt_ids = manual.flatMap((claim, index) =>
				Option.isNone(claim) ? [pending[index]!.attempt_id] : [],
			);

			return {
				completed_attempt_ids: attempts
					.filter(({ outcome }) => outcome === "completed")
					.map(({ attempt_id }) => attempt_id),
				deferred_attempt_ids: [
					...deferred_manual_attempt_ids,
					...attempts
						.filter(({ outcome }) => outcome === "deferred")
						.map(({ attempt_id }) => attempt_id),
				],
			} satisfies WorkspaceGitFetchCycleResult;
		});
		const RunOnce = Semaphore.withPermit(cycle_lock)(RunOnceUnlocked);
		const DispatchLoop = Effect.gen(function* () {
			while (true) {
				const cycle = yield* RunOnce.pipe(Effect.result);
				const retry =
					Result.isFailure(cycle) || cycle.success.deferred_attempt_ids.length > 0;
				const continue_dispatch = yield* SubscriptionRef.modify(dispatch_state, (state) => {
					const requested = state === "pending";
					const keep_running = retry || requested;

					return [keep_running, keep_running ? "running" : "idle"] as const;
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
		const QuiesceThread = (thread_id: string) => dispatch_fence.Quiesce(thread_id, Effect.void);
		const UpdatePolicy = (input: WorkspaceGitFetchPolicyUpdateInput) =>
			Schema.decodeUnknownEffect(FetchPolicyUpdate, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError((cause) => service_failure("invalid_request", cause)),
				Effect.flatMap((decoded) =>
					Hash(policy_fingerprint(decoded)).pipe(
						Effect.flatMap((request_fingerprint) =>
							repository.UpdatePolicy({ ...decoded, request_fingerprint }),
						),
					),
				),
				Effect.tap((acceptance) =>
					acceptance.policy.enabled ? WakeDispatcher : Effect.void,
				),
			);
		const Request = (input: WorkspaceGitFetchRequestInput) =>
			Schema.decodeUnknownEffect(ManualFetchRequest, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError((cause) => service_failure("invalid_request", cause)),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const request_fingerprint = yield* Hash(manual_fingerprint(decoded));
						const existing = yield* repository.ReadManual(decoded.message_id);
						const attempt_id = Option.isSome(existing)
							? existing.value.attempt_id
							: yield* metadata.MakeId("fetch");
						const acceptance = yield* repository.PrepareManual({
							...decoded,
							attempt_id,
							request_fingerprint,
						});

						yield* acceptance.operation.status === "pending"
							? WakeDispatcher
							: Effect.void;

						return acceptance;
					}),
				),
			);

		yield* WakeDispatcher;
		yield* Effect.forkIn(
			scheduler.Schedule(WakeDispatcher).pipe(Scope.provide(service_scope)),
			service_scope,
		);
		yield* Effect.yieldNow;

		return {
			AwaitIdle,
			Query: repository.Query,
			QuiesceThread,
			Request,
			RunOnce,
			UpdatePolicy,
		};
	}),
);

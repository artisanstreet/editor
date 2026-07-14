import {
	Cause,
	Context,
	Data,
	DateTime,
	Effect,
	Exit,
	Layer,
	Option,
	Result,
	Scope,
	Semaphore,
} from "effect";

import type { ExternalWaitState, ExternalWaitTarget } from "@artisan/protocol";

import { EvaluateExternalWait } from "./external-wait-policy";
import {
	ExternalWaitRepository,
	type ExternalWaitObservationClaim,
	type ExternalWaitRepositoryError,
} from "./external-wait-repository";
import {
	GitProviderError,
	type GitProviderPullRequestTargetRead,
} from "../git-provider/git-provider";
import { GitProviderRegistry } from "../git-provider/git-provider-registry";
import { ProjectRepository, type ProjectRepositoryError } from "../projects/project-repository";
import type { RegisteredProject } from "../projects/project";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

const observation_interval_seconds = 15;
const provider_timeout = "20 seconds";

type SuspensionReason = Extract<ExternalWaitState, { readonly _tag: "suspended" }>["reason"];

export interface ExternalWaitCycleResult {
	readonly observed_wait_ids: ReadonlyArray<string>;
	readonly reconciled_wait_ids: ReadonlyArray<string>;
}

/** Owns the scoped periodic cadence for external provider observations. */
export class ExternalWaitScheduler extends Context.Service<
	ExternalWaitScheduler,
	{
		readonly Schedule: (task: Effect.Effect<void>) => Effect.Effect<never, never, Scope.Scope>;
	}
>()("Artisan/ExternalWaitScheduler") {}

/** Reports an internal observation failure that should be retried after lease expiry. */
export class ExternalWaitCoordinatorFailure extends Data.TaggedError(
	"ExternalWaitCoordinatorFailure",
)<{
	readonly cause: ExternalWaitRepositoryError | ProjectRepositoryError | unknown;
	readonly wait_id?: string;
}> {}

/** Observes exact hosted state and advances durable external waits without model polling. */
export class ExternalWaitCoordinator extends Context.Service<
	ExternalWaitCoordinator,
	{
		readonly RunOnce: Effect.Effect<ExternalWaitCycleResult, ExternalWaitCoordinatorFailure>;
	}
>()("Artisan/ExternalWaitCoordinator") {}

/** Runs production observation cycles at the repository's fifteen-second cadence. */
export const ExternalWaitSchedulerLive = Layer.succeed(ExternalWaitScheduler, {
	Schedule: (task) => Effect.forever(Effect.sleep("15 seconds").pipe(Effect.andThen(task))),
});

function CoordinatorFailure(cause: unknown, wait_id?: string) {
	return new ExternalWaitCoordinatorFailure({
		cause,
		...(wait_id === undefined ? {} : { wait_id }),
	});
}

function ParseInstant(value: string, label: string) {
	return Option.match(DateTime.make(value), {
		onNone: () => Effect.fail(CoordinatorFailure(new Error(`${label} is invalid`))),
		onSome: Effect.succeed,
	});
}

const HasExpired = (expires_at: string, now: string) =>
	Effect.gen(function* () {
		const expiry = yield* ParseInstant(expires_at, "External wait timeout");
		const current = yield* ParseInstant(now, "External wait clock");

		return DateTime.toEpochMillis(expiry) <= DateTime.toEpochMillis(current);
	});

const NextObservationAt = (now: string) =>
	Effect.map(ParseInstant(now, "External wait clock"), (current) =>
		DateTime.formatIso(DateTime.add(current, { seconds: observation_interval_seconds })),
	);

function project_matches_target(
	project: RegisteredProject,
	target: ExternalWaitTarget,
	workspace_id: string,
) {
	const origin = project.hosted_origin;
	const repository = target.repository;

	return (
		project.workspace_id === workspace_id &&
		origin.canonical_host === repository.host &&
		origin.name === repository.name &&
		origin.owner === repository.owner &&
		origin.provider_id === repository.provider_id
	);
}

function provider_suspension(error: unknown): SuspensionReason {
	if (Cause.isTimeoutError(error)) {
		return "timeout";
	}

	if (!(error instanceof GitProviderError)) {
		return "provider_unavailable";
	}

	if (error.reason === "auth_required" || error.reason === "account_not_active") {
		return "authentication_required";
	}

	if (error.reason === "rate_limited") {
		return "rate_limited";
	}

	if (error.reason === "not_found" || error.reason === "stale_repository") {
		return "project_unavailable";
	}

	return error.reason === "timed_out" ? "timeout" : "provider_unavailable";
}

/** Supplies scoped external-wait observation and source-closure reconciliation. */
export const ExternalWaitCoordinatorLive = Layer.effect(
	ExternalWaitCoordinator,
	Effect.gen(function* () {
		const external_waits = yield* ExternalWaitRepository;
		const metadata = yield* RuntimeMetadata;
		const projects = yield* ProjectRepository;
		const providers = yield* GitProviderRegistry;
		const scheduler = yield* ExternalWaitScheduler;
		const service_scope = yield* Scope.make();
		const cycle_lock = yield* Semaphore.make(1);

		const RecordSuspension = (
			claim: ExternalWaitObservationClaim,
			lease_owner: string,
			now: string,
			reason: SuspensionReason,
		) =>
			external_waits.RecordObservation({
				lease_owner,
				next_observation_at: now,
				now,
				state: { _tag: "suspended", reason },
				wait_id: claim.snapshot.wait_id,
			});

		const ObserveClaim = (
			claim: ExternalWaitObservationClaim,
			lease_owner: string,
			claimed_at: string,
		) =>
			Effect.gen(function* () {
				const wait_id = claim.snapshot.wait_id;

				if (yield* HasExpired(claim.timeout_at, claimed_at)) {
					yield* RecordSuspension(claim, lease_owner, claimed_at, "timeout");

					return;
				}

				const project = yield* projects
					.FindByProjectId({ project_id: claim.snapshot.project_id })
					.pipe(Effect.mapError((cause) => CoordinatorFailure(cause, wait_id)));

				if (
					Option.isNone(project) ||
					!project_matches_target(
						project.value,
						claim.snapshot.target,
						claim.snapshot.workspace_id,
					)
				) {
					yield* RecordSuspension(
						claim,
						lease_owner,
						yield* metadata.Now,
						"project_unavailable",
					);

					return;
				}

				const provider = yield* providers
					.Get(claim.snapshot.target.repository.provider_id)
					.pipe(Effect.result);

				if (
					Result.isFailure(provider) ||
					provider.success.ReadPullRequestTarget === undefined
				) {
					yield* RecordSuspension(
						claim,
						lease_owner,
						yield* metadata.Now,
						"provider_unavailable",
					);

					return;
				}

				const ReadPullRequestTarget = provider.success.ReadPullRequestTarget;
				const target = claim.snapshot.target;
				const read_input = {
					expected_head: target.expected_head_commit,
					pull_request_number: target.pull_request_number,
					pull_request_origin: target.pull_request_origin,
					repository: target.repository,
					selected_branch: target.branch,
					selection: {
						account_login: project.value.hosted_origin.selected_account_login,
						host: target.repository.host,
						provider_id: target.repository.provider_id,
					},
				} satisfies GitProviderPullRequestTargetRead;
				const lookup = yield* ReadPullRequestTarget(read_input).pipe(
					Effect.timeout(provider_timeout),
					Effect.result,
				);
				const observed_at = yield* metadata.Now;

				if (yield* HasExpired(claim.timeout_at, observed_at)) {
					yield* RecordSuspension(claim, lease_owner, observed_at, "timeout");

					return;
				}

				if (Result.isFailure(lookup)) {
					yield* RecordSuspension(
						claim,
						lease_owner,
						observed_at,
						provider_suspension(lookup.failure),
					);

					return;
				}

				const evaluation = yield* EvaluateExternalWait({
					baseline: claim.baseline,
					lookup: lookup.success,
				}).pipe(Effect.result);

				if (Result.isFailure(evaluation)) {
					yield* RecordSuspension(
						claim,
						lease_owner,
						observed_at,
						"provider_unavailable",
					);

					return;
				}

				if (evaluation.success._tag === "wake") {
					yield* external_waits.CreateWake({
						lease_owner,
						now: observed_at,
						trigger: evaluation.success.trigger,
						wait_id,
					});

					return;
				}

				if (evaluation.success._tag === "suspend") {
					yield* RecordSuspension(
						claim,
						lease_owner,
						observed_at,
						evaluation.success.reason,
					);

					return;
				}

				yield* external_waits.ReleaseObservation({
					lease_owner,
					next_observation_at: yield* NextObservationAt(observed_at),
					now: observed_at,
					state: { _tag: "waiting" },
					wait_id,
				});
			}).pipe(Effect.mapError((cause) => CoordinatorFailure(cause, claim.snapshot.wait_id)));

		const ObserveWait = (wait_id: string, now: string) =>
			Effect.gen(function* () {
				const claim = yield* external_waits.ClaimObservation({
					lease_owner: metadata.instance_id,
					now,
					wait_id,
				});

				if (Option.isNone(claim)) {
					return Option.none<string>();
				}

				yield* ObserveClaim(claim.value, metadata.instance_id, now);

				return Option.some(wait_id);
			}).pipe(Effect.mapError((cause) => CoordinatorFailure(cause, wait_id)));

		const RunOnceUnlocked = Effect.gen(function* () {
			const now = yield* metadata.Now;
			const reconciled_wait_ids = yield* external_waits.ReconcileSourceClosures({ now });
			const due_wait_ids = yield* external_waits.DiscoverDueObservations({ now });
			const observed = yield* Effect.forEach(
				due_wait_ids,
				(wait_id) => ObserveWait(wait_id, now),
				{ concurrency: 4 },
			);

			return {
				observed_wait_ids: observed.filter(Option.isSome).map(({ value }) => value),
				reconciled_wait_ids,
			} satisfies ExternalWaitCycleResult;
		}).pipe(Effect.mapError((cause) => CoordinatorFailure(cause)));
		const RunOnce = Semaphore.withPermit(cycle_lock)(RunOnceUnlocked);

		yield* RunOnce;
		yield* Effect.addFinalizer(() => Scope.close(service_scope, Exit.void));
		yield* Effect.forkIn(
			scheduler
				.Schedule(
					RunOnce.pipe(
						Effect.asVoid,
						Effect.catch(() => Effect.void),
					),
				)
				.pipe(Scope.provide(service_scope)),
			service_scope,
		);
		yield* Effect.yieldNow;

		return { RunOnce };
	}),
);

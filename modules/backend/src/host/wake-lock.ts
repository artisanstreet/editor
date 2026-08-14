import { and, eq, inArray } from "drizzle-orm";
import { Clock, Context, Data, Effect, Layer, PubSub, Ref } from "effect";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { AgentRuns, OrchestrationInteractions, OrchestrationRuns } from "../persistence/tables";
import {
	assess_unsettled_work,
	type UnsettledWorkSnapshot,
	type WakeLockAssessment,
} from "./wake-lock-policy";

/** The host refused or cannot express a power assertion; the lock stays best-effort. @since 0.7.0 */
export class SystemWakeLockUnavailable extends Data.TaggedError("SystemWakeLockUnavailable")<{
	readonly message: string;
	readonly cause?: unknown;
}> {}

/** One acquired OS power assertion. Releasing twice must be harmless. @since 0.7.0 */
export interface SystemWakeLockHandle {
	readonly Release: Effect.Effect<void>;
}

/**
 * Platform seam for the one OS-level "system required" assertion. The service
 * above it owns when to hold; a backend only knows how.
 *
 * @since 0.7.0
 */
export class SystemWakeLockBackend extends Context.Service<
	SystemWakeLockBackend,
	{
		/** The reason string surfaces in OS tooling (`powercfg /requests`, `pmset -g assertions`). */
		readonly Acquire: (
			reason: string,
		) => Effect.Effect<SystemWakeLockHandle, SystemWakeLockUnavailable>;
	}
>()("Artisan/SystemWakeLockBackend") {}

/** Point-in-time wake-lock state, for logging and UI surfacing. @since 0.7.0 */
export interface SystemWakeLockStatus {
	readonly held: boolean;
	readonly held_count: number;
}

/**
 * Keeps the host awake for exactly as long as Artisan owns unsettled work.
 *
 * The lock is derived, never commanded: a predicate over durable run and queue
 * state drives acquisition and release, re-evaluated on every journal commit
 * and on a slow backstop poll. It prevents system sleep only — display sleep
 * and session lock stay untouched — and it is best-effort: a hold that cannot
 * be acquired is logged and ignored, never failing a run.
 *
 * @since 0.7.0
 */
export class SystemWakeLock extends Context.Service<
	SystemWakeLock,
	{
		readonly Status: Effect.Effect<SystemWakeLockStatus>;
	}
>()("Artisan/SystemWakeLock") {}

/** Configures the wake-lock decision cadence. @since 0.7.0 */
export interface SystemWakeLockOptions {
	/** How long a human-blocked run keeps the host awake after its newest request. */
	readonly approval_grace_ms?: number;
	/** Hold retained after the count reaches zero so a draining queue does not thrash the OS handle. */
	readonly linger_ms?: number;
	/** Backstop re-evaluation spacing when no journal commit arrives. */
	readonly poll_interval_ms?: number;
	/** Human-readable justification handed to the OS. */
	readonly reason?: string;
	/** Test seam replacing the database-backed unsettled-work read. */
	readonly EvaluateUnsettledWork?: Effect.Effect<UnsettledWorkSnapshot>;
}

const empty_snapshot: UnsettledWorkSnapshot = {
	approval_requested_at_ms: [],
	progressing_count: 0,
};

/**
 * Reads the two durable sources of unsettled work: run lifecycle rows, and
 * graph work whose dispatch has not produced a lifecycle event yet. A waiting
 * run with a pending approval or question is blocked on a human and reported
 * separately so the policy can age it out; a waiting run without one is a
 * pending retry that progresses on its own.
 */
const ReadUnsettledWork = (database: typeof Database.Service) =>
	Effect.gen(function* () {
		const live_runs = yield* database.client
			.select({ run_id: OrchestrationRuns.run_id, status: OrchestrationRuns.status })
			.from(OrchestrationRuns)
			.where(inArray(OrchestrationRuns.status, ["queued", "running", "waiting"]));
		const waiting_run_ids = live_runs
			.filter((run) => run.status === "waiting")
			.map((run) => run.run_id);
		const pending_interactions =
			waiting_run_ids.length === 0
				? []
				: yield* database.client
						.select({
							created_at: OrchestrationInteractions.created_at,
							run_id: OrchestrationInteractions.run_id,
						})
						.from(OrchestrationInteractions)
						.where(
							and(
								eq(OrchestrationInteractions.state, "requested"),
								inArray(OrchestrationInteractions.run_id, waiting_run_ids),
							),
						);
		const undispatched_graph_runs = yield* database.client
			.select({ run_id: AgentRuns.run_id })
			.from(AgentRuns)
			.where(inArray(AgentRuns.dispatch_status, ["queued", "dispatching"]));

		const newest_request_by_run = new Map<string, number>();

		for (const interaction of pending_interactions) {
			const requested_at_ms = Date.parse(interaction.created_at);

			if (Number.isNaN(requested_at_ms)) continue;

			newest_request_by_run.set(
				interaction.run_id,
				Math.max(newest_request_by_run.get(interaction.run_id) ?? 0, requested_at_ms),
			);
		}

		const progressing_waiting = waiting_run_ids.filter(
			(run_id) => !newest_request_by_run.has(run_id),
		).length;

		return {
			approval_requested_at_ms: [...newest_request_by_run.values()],
			progressing_count:
				live_runs.length -
				waiting_run_ids.length +
				progressing_waiting +
				undispatched_graph_runs.length,
		} satisfies UnsettledWorkSnapshot;
	});

interface DriverState {
	readonly acquire_warned: boolean;
	readonly handle: SystemWakeLockHandle | undefined;
	readonly held_count: number;
	/** Instant the count reached zero while the handle lingers, if it has. */
	readonly zero_since_ms: number | undefined;
}

const initial_driver_state: DriverState = {
	acquire_warned: false,
	handle: undefined,
	held_count: 0,
	zero_since_ms: undefined,
};

/** @since 0.7.0 */
export const MakeSystemWakeLockLayer = (options: SystemWakeLockOptions = {}) =>
	Layer.effect(
		SystemWakeLock,
		Effect.gen(function* () {
			const database = yield* Database;
			const notifier = yield* JournalNotifier;
			const backend = yield* SystemWakeLockBackend;
			const configured = {
				approval_grace_ms: options.approval_grace_ms ?? 5 * 60_000,
				linger_ms: options.linger_ms ?? 60_000,
				poll_interval_ms: options.poll_interval_ms ?? 30_000,
				reason: options.reason ?? "Artisan is running queued agent work",
			};
			const Evaluate: Effect.Effect<UnsettledWorkSnapshot, unknown> =
				options.EvaluateUnsettledWork ?? ReadUnsettledWork(database);
			const state = yield* Ref.make(initial_driver_state);

			/** Registered before the driver forks, so it runs after the driver stops. */
			yield* Effect.addFinalizer(() =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);

					if (current.handle !== undefined) {
						yield* current.handle.Release;
						yield* Effect.logInfo("Released the system wake lock on shutdown");
					}
				}),
			);

			const subscription = yield* notifier.Subscribe;

			const Acquire = (assessment: WakeLockAssessment, current: DriverState) =>
				Effect.gen(function* () {
					const handle = yield* backend.Acquire(configured.reason).pipe(
						Effect.tap(() =>
							Effect.logInfo("Holding the system awake", {
								held_count: assessment.held_count,
							}),
						),
						Effect.catch((unavailable) =>
							(current.acquire_warned
								? Effect.logDebug("System wake lock still unavailable", {
										message: unavailable.message,
									})
								: Effect.logWarning("Could not hold the system awake", {
										cause: unavailable.cause,
										message: unavailable.message,
									})
							).pipe(Effect.as(undefined)),
						),
					);

					yield* Ref.set(state, {
						acquire_warned: handle === undefined,
						handle,
						held_count: assessment.held_count,
						zero_since_ms: undefined,
					});
				});

			const Apply = (assessment: WakeLockAssessment, now_ms: number) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(state);

					if (assessment.hold) {
						if (current.handle === undefined) {
							return yield* Acquire(assessment, current);
						}

						return yield* Ref.set(state, {
							...current,
							held_count: assessment.held_count,
							zero_since_ms: undefined,
						});
					}

					if (current.handle === undefined) {
						return yield* Ref.set(state, { ...initial_driver_state });
					}

					const zero_since_ms = current.zero_since_ms ?? now_ms;

					if (now_ms - zero_since_ms < configured.linger_ms) {
						return yield* Ref.set(state, { ...current, held_count: 0, zero_since_ms });
					}

					yield* current.handle.Release;
					yield* Effect.logInfo("Released the system wake lock", {
						lingered_ms: now_ms - zero_since_ms,
					});
					yield* Ref.set(state, { ...initial_driver_state });
				});

			const Drive = Effect.gen(function* () {
				const now_ms = yield* Clock.currentTimeMillis;
				const snapshot = yield* Evaluate.pipe(
					Effect.catch((cause) =>
						Effect.logWarning("Could not measure unsettled work for the wake lock", {
							cause,
						}).pipe(Effect.as(empty_snapshot)),
					),
				);
				const assessment = assess_unsettled_work(
					snapshot,
					now_ms,
					configured.approval_grace_ms,
				);

				yield* Apply(assessment, now_ms);

				const after = yield* Ref.get(state);
				const linger_deadline_ms =
					after.handle !== undefined && after.zero_since_ms !== undefined
						? after.zero_since_ms + configured.linger_ms
						: undefined;
				const wait_ms = Math.min(
					configured.poll_interval_ms,
					...[assessment.recheck_at_ms, linger_deadline_ms]
						.filter((deadline_ms) => deadline_ms !== undefined)
						.map((deadline_ms) => deadline_ms - now_ms),
				);

				yield* Effect.race(
					PubSub.take(subscription).pipe(Effect.asVoid),
					Effect.sleep(Math.max(wait_ms, 1_000)),
				);
			}).pipe(Effect.forever);

			yield* Effect.forkScoped(Drive);

			return {
				Status: Ref.get(state).pipe(
					Effect.map((current) => ({
						held: current.handle !== undefined,
						held_count: current.held_count,
					})),
				),
			};
		}),
	);

/** @since 0.7.0 */
export const SystemWakeLockLive = MakeSystemWakeLockLayer();

/** Inert default for compositions without a platform backend: never holds, never polls. @since 0.7.0 */
export const SystemWakeLockDisabled = Layer.succeed(SystemWakeLock, {
	Status: Effect.succeed({ held: false, held_count: 0 }),
});

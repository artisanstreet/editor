import {
	Context,
	Data,
	Deferred,
	Effect,
	Fiber,
	Layer,
	Option,
	Ref,
	Result,
	Semaphore,
	Stream,
	SubscriptionRef,
} from "effect";

import type {
	EngineInstallationMutationResult,
	EngineInstallationQuery,
	EngineInstallationReport,
	EngineInstallationSnapshot,
} from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";

const installation_poll_delay = "750 millis";
const installation_poll_timeout = "90 seconds";

export class EngineInstallationRejected extends Data.TaggedError("EngineInstallationRejected")<{
	readonly engine_id: string;
	readonly message: string;
}> {}

export class EngineInstallationTimedOut extends Data.TaggedError("EngineInstallationTimedOut")<{
	readonly engine_id: string;
}> {}

class EngineInstallationMonitorReplaced extends Data.TaggedError(
	"EngineInstallationMonitorReplaced",
)<{
	readonly engine_id: string;
}> {}

export type EngineInstallationControllerError =
	| ArtisanClientError
	| EngineInstallationRejected
	| EngineInstallationTimedOut;

export interface EngineInstallationsState {
	readonly available: boolean;
	readonly errors: Readonly<Record<string, string>>;
	readonly fetched_at?: string;
	readonly load_error?: string | undefined;
	readonly pending_engine_ids: ReadonlySet<string>;
	readonly reports: Readonly<Record<string, EngineInstallationReport>>;
}

const InitialState: EngineInstallationsState = {
	available: false,
	errors: {},
	pending_engine_ids: new Set(),
	reports: {},
};

type RefreshFlight = {
	readonly deferred: Deferred.Deferred<EngineInstallationsState, ArtisanClientError>;
};

type InstallationMonitor = {
	readonly fiber: Fiber.Fiber<void, never>;
	readonly generation: number;
};

const RefreshKey = (input: EngineInstallationQuery | undefined) =>
	JSON.stringify({
		check_updates: input?.check_updates === true,
		engine_id: input?.engine_id,
	});

const MergeSnapshot = (
	state: EngineInstallationsState,
	snapshot: EngineInstallationSnapshot,
	AcceptReport: (report: EngineInstallationReport) => boolean = () => true,
): EngineInstallationsState => {
	const reports = { ...state.reports };
	const errors = { ...state.errors };

	for (const report of snapshot.engines) {
		if (!AcceptReport(report)) continue;
		reports[report.engine_id] = report;
		/** A terminal backend report supersedes any local mutation/timeout message. */
		if (report.activity === "idle" || report.activity === "failed") {
			delete errors[report.engine_id];
		}
	}

	return {
		...state,
		available: true,
		errors,
		fetched_at: snapshot.fetched_at,
		load_error: undefined,
		reports,
	};
};

const LoadFailed = (state: EngineInstallationsState): EngineInstallationsState => ({
	...state,
	load_error: "Installation status could not be loaded. Try again.",
});

const MergeReport = (
	state: EngineInstallationsState,
	report: EngineInstallationReport,
): EngineInstallationsState => ({
	...state,
	available: true,
	reports: { ...state.reports, [report.engine_id]: report },
});

const Pending = (state: EngineInstallationsState, engine_id: string): EngineInstallationsState => ({
	...state,
	errors: Object.fromEntries(
		Object.entries(state.errors).filter(([candidate]) => candidate !== engine_id),
	),
	pending_engine_ids: new Set([...state.pending_engine_ids, engine_id]),
});

const Settled = (
	state: EngineInstallationsState,
	engine_id: string,
	message?: string,
): EngineInstallationsState => {
	const pending_engine_ids = new Set(state.pending_engine_ids);
	pending_engine_ids.delete(engine_id);
	const errors = Object.fromEntries(
		Object.entries(state.errors).filter(([candidate]) => candidate !== engine_id),
	);

	return {
		...state,
		errors: message === undefined ? errors : { ...errors, [engine_id]: message },
		pending_engine_ids,
	};
};

export class EngineInstallationsController extends Context.Service<
	EngineInstallationsController,
	{
		readonly Changes: Stream.Stream<EngineInstallationsState>;
		readonly Authenticate: (
			engine_id: string,
		) => Effect.Effect<EngineInstallationsState, EngineInstallationControllerError>;
		readonly Current: Effect.Effect<EngineInstallationsState>;
		readonly Install: (
			engine_id: string,
			version?: string,
		) => Effect.Effect<EngineInstallationsState, EngineInstallationControllerError>;
		readonly Refresh: (
			input?: EngineInstallationQuery,
		) => Effect.Effect<EngineInstallationsState, ArtisanClientError>;
		readonly Rollback: (
			engine_id: string,
		) => Effect.Effect<EngineInstallationsState, EngineInstallationControllerError>;
	}
>()("Artisan/EngineInstallationsController") {}

export const EngineInstallationsControllerLive = Layer.effect(
	EngineInstallationsController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const state = yield* SubscriptionRef.make<EngineInstallationsState>(InitialState);
		const mutation_locks = yield* Ref.make<ReadonlyMap<string, Semaphore.Semaphore>>(new Map());
		const monitor_generations = yield* Ref.make<ReadonlyMap<string, number>>(new Map());
		const monitors = yield* Ref.make<ReadonlyMap<string, InstallationMonitor>>(new Map());
		const refresh_flights = yield* Ref.make<ReadonlyMap<string, RefreshFlight>>(new Map());

		const Current = Effect.gen(function* () {
			return yield* SubscriptionRef.get(state);
		});
		const Refresh = (input?: EngineInstallationQuery) =>
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					const key = RefreshKey(input);
					const deferred = yield* Deferred.make<
						EngineInstallationsState,
						ArtisanClientError
					>();
					const claimed = yield* Ref.modify(refresh_flights, (current) => {
						const active = current.get(key);
						if (active !== undefined) return [active, current] as const;
						const flight = { deferred } satisfies RefreshFlight;
						return [flight, new Map(current).set(key, flight)] as const;
					});

					if (claimed.deferred !== deferred)
						return yield* restore(Deferred.await(claimed.deferred));

					const generations_at_start = yield* Ref.get(monitor_generations);
					const CompleteRefresh = client.GetEngineInstallations(input).pipe(
						Effect.tapError(() => SubscriptionRef.update(state, LoadFailed)),
						Effect.flatMap((snapshot) =>
							Ref.get(monitor_generations).pipe(
								Effect.flatMap((current_generations) =>
									SubscriptionRef.updateAndGet(state, (current) =>
										MergeSnapshot(
											current,
											snapshot,
											(report) =>
												generations_at_start.get(report.engine_id) ===
												current_generations.get(report.engine_id),
										),
									),
								),
							),
						),
						Effect.exit,
						Effect.flatMap((exit) =>
							Effect.gen(function* () {
								yield* Ref.update(refresh_flights, (current) => {
									if (current.get(key) !== claimed) return current;
									const next = new Map(current);
									next.delete(key);
									return next;
								});
								yield* Deferred.done(claimed.deferred, exit);
							}),
						),
						Effect.asVoid,
					);
					yield* Effect.forkIn(CompleteRefresh, controller_scope);
					return yield* restore(Deferred.await(deferred));
				}),
			);

		const RecordFailure = (engine_id: string, message: string) =>
			Effect.gen(function* () {
				return yield* SubscriptionRef.updateAndGet(state, (current) =>
					Settled(current, engine_id, message),
				);
			});

		const IsCurrentMonitor = (engine_id: string, generation: number) =>
			Ref.get(monitor_generations).pipe(
				Effect.map((current) => current.get(engine_id) === generation),
			);

		/**
		 * A monitor owns its query result until it has checked its generation. A
		 * replacement command therefore cannot be settled by a late response from
		 * the preceding operation.
		 */
		const RefreshMonitor = (engine_id: string, generation: number) =>
			Effect.gen(function* () {
				const snapshot = yield* client.GetEngineInstallations({ engine_id });
				if (!(yield* IsCurrentMonitor(engine_id, generation))) return Option.none();
				return Option.some(
					yield* SubscriptionRef.updateAndGet(state, (current) =>
						MergeSnapshot(current, snapshot),
					),
				);
			});

		const AwaitTerminal = (engine_id: string, generation: number) => {
			const Poll = (): Effect.Effect<
				EngineInstallationsState,
				ArtisanClientError | EngineInstallationMonitorReplaced
			> =>
				Effect.gen(function* () {
					const refreshed = yield* RefreshMonitor(engine_id, generation);
					if (Option.isNone(refreshed))
						return yield* Effect.fail(
							new EngineInstallationMonitorReplaced({ engine_id }),
						);
					const current = refreshed.value;
					const report = current.reports[engine_id];

					if (
						report?.activity === "installing" ||
						report?.activity === "authenticating"
					) {
						yield* Effect.sleep(installation_poll_delay);
						return yield* Poll();
					}

					return current;
				});

			return Effect.gen(function* () {
				const completed = yield* Poll().pipe(
					Effect.timeoutOption(installation_poll_timeout),
				);

				if (Option.isNone(completed)) {
					return yield* Effect.fail(new EngineInstallationTimedOut({ engine_id }));
				}

				return completed.value;
			});
		};

		/**
		 * Claims a fresh generation before publishing a command receipt. Late
		 * route refreshes and superseded monitor reads can then never overwrite
		 * that receipt, even if their transport reply arrives afterwards.
		 */
		const ReplaceMonitor = (engine_id: string) =>
			Effect.gen(function* () {
				const generation = yield* Ref.modify(monitor_generations, (current) => {
					const next_generation = (current.get(engine_id) ?? 0) + 1;
					return [
						next_generation,
						new Map(current).set(engine_id, next_generation),
					] as const;
				});
				const prior_monitor = yield* Ref.modify(monitors, (current) => {
					const prior = current.get(engine_id);
					if (prior === undefined) return [undefined, current] as const;
					const next = new Map(current);
					next.delete(engine_id);
					return [prior, next] as const;
				});
				if (prior_monitor !== undefined)
					yield* Effect.forkIn(Fiber.interrupt(prior_monitor.fiber), controller_scope);
				return generation;
			});

		const StartMonitor = (engine_id: string, generation: number) =>
			Effect.gen(function* () {
				const Monitor = (): Effect.Effect<void, never> =>
					AwaitTerminal(engine_id, generation).pipe(
						Effect.result,
						Effect.flatMap((outcome) =>
							IsCurrentMonitor(engine_id, generation).pipe(
								Effect.flatMap((current) => {
									/** A replacement owns settlement and has already started its monitor. */
									if (!current) return Effect.void;
									if (Result.isFailure(outcome)) {
										const failure = outcome.failure;
										return SubscriptionRef.update(state, (state) =>
											Settled(
												state,
												engine_id,
												failure._tag === "EngineInstallationTimedOut"
													? "The installation is still running. Check back shortly."
													: failure.message,
											),
										);
									}
									return SubscriptionRef.update(state, (state) =>
										Settled(
											state,
											engine_id,
											outcome.success.reports[engine_id]?.failure,
										),
									);
								}),
							),
						),
					);

				const start = yield* Deferred.make<void>();
				const MonitorWithCleanup = Deferred.await(start).pipe(
					Effect.flatMap(() => Monitor()),
					Effect.ensuring(
						Ref.update(monitors, (current) => {
							if (current.get(engine_id)?.generation !== generation) return current;
							const next = new Map(current);
							next.delete(engine_id);
							return next;
						}),
					),
				);
				const fiber = yield* Effect.forkIn(MonitorWithCleanup, controller_scope);
				yield* Ref.update(monitors, (current) =>
					new Map(current).set(engine_id, { fiber, generation }),
				);
				yield* Deferred.succeed(start, undefined);
			});

		const MutationLock = (engine_id: string) =>
			Ref.modify(mutation_locks, (current) => {
				const existing = current.get(engine_id);
				if (existing !== undefined) return [existing, current] as const;
				const created = Semaphore.makeUnsafe(1);
				return [created, new Map(current).set(engine_id, created)] as const;
			});

		const Mutate = (
			engine_id: string,
			Request: Effect.Effect<EngineInstallationMutationResult, ArtisanClientError>,
		) =>
			Effect.flatMap(MutationLock(engine_id), (mutation_lock) =>
				mutation_lock.withPermit(
					Effect.gen(function* () {
						yield* SubscriptionRef.update(state, (current) =>
							Pending(current, engine_id),
						);
						const result = yield* Request.pipe(
							Effect.tapError((failure) => RecordFailure(engine_id, failure.message)),
						);

						if (result.status === "rejected") {
							const failure = new EngineInstallationRejected({
								engine_id,
								message: result.message,
							});
							yield* RecordFailure(engine_id, failure.message);
							return yield* Effect.fail(failure);
						}

						const generation = yield* ReplaceMonitor(engine_id);
						yield* SubscriptionRef.update(state, (current) =>
							MergeReport(current, result.report),
						);
						if (
							result.report.activity === "installing" ||
							result.report.activity === "authenticating"
						) {
							yield* StartMonitor(engine_id, generation);
							return yield* Current;
						}
						return yield* SubscriptionRef.updateAndGet(state, (current) =>
							Settled(current, engine_id, result.report.failure),
						);
					}),
				),
			);

		const Install = (engine_id: string, version?: string) =>
			Mutate(
				engine_id,
				client.InstallEngine({ engine_id, ...(version === undefined ? {} : { version }) }),
			);

		const Rollback = (engine_id: string) =>
			Mutate(engine_id, client.RollbackEngine({ engine_id }));

		const Authenticate = (engine_id: string) =>
			Mutate(engine_id, client.AuthenticateEngine({ engine_id }));

		return EngineInstallationsController.of({
			Authenticate,
			Changes: SubscriptionRef.changes(state),
			Current,
			Install,
			Refresh,
			Rollback,
		});
	}),
);

import { describe, expect, it } from "vitest";
import { Effect, Layer, Ref } from "effect";
import { TestClock } from "effect/testing";

import {
	MakeSystemWakeLockLayer,
	SystemWakeLock,
	SystemWakeLockBackend,
	SystemWakeLockUnavailable,
} from "../../modules/backend/src/host/wake-lock";
import type { UnsettledWorkSnapshot } from "../../modules/backend/src/host/wake-lock-policy";
import { Database, type DatabaseClient } from "../../modules/backend/src/persistence/database";
import {
	JournalNotifier,
	JournalNotifierLive,
} from "../../modules/backend/src/persistence/journal-notifier";

const idle: UnsettledWorkSnapshot = { approval_requested_at_ms: [], progressing_count: 0 };
const one_running: UnsettledWorkSnapshot = { approval_requested_at_ms: [], progressing_count: 1 };

const approval_grace_ms = 5_000;
const linger_ms = 2_000;
const poll_interval_ms = 1_000;

interface ScenarioContext {
	readonly events: Effect.Effect<ReadonlyArray<string>>;
	readonly SetSnapshot: (snapshot: UnsettledWorkSnapshot) => Effect.Effect<void>;
	/** Simulates a durable journal commit waking the derivation. */
	readonly Signal: Effect.Effect<void>;
	readonly status: (typeof SystemWakeLock.Service)["Status"];
}

/**
 * Runs the wake-lock driver against a scripted work snapshot and a recording
 * backend on the test clock. The snapshot Ref replaces the database read, so
 * the untested surface is exactly the SQL projection, not the derivation.
 */
const run_scenario = <A>(
	initial: UnsettledWorkSnapshot,
	Script: (context: ScenarioContext) => Effect.Effect<A>,
	options: { unavailable?: boolean } = {},
) =>
	Effect.runPromise(
		Effect.gen(function* () {
			const events = yield* Ref.make<ReadonlyArray<string>>([]);
			const snapshot = yield* Ref.make(initial);
			const backend = Layer.succeed(SystemWakeLockBackend, {
				Acquire: () =>
					options.unavailable === true
						? Effect.fail(
								new SystemWakeLockUnavailable({ message: "scripted refusal" }),
							)
						: Ref.update(events, (current) => [...current, "acquire"]).pipe(
								Effect.as({
									Release: Ref.update(events, (current) => [
										...current,
										"release",
									]),
								}),
							),
			});
			/** Never dereferenced: the scripted snapshot replaces every database read. */
			const database = Layer.succeed(Database, {
				client: undefined as unknown as DatabaseClient,
			});
			const wake_lock = MakeSystemWakeLockLayer({
				approval_grace_ms,
				linger_ms,
				poll_interval_ms,
				EvaluateUnsettledWork: Ref.get(snapshot),
			}).pipe(
				Layer.provideMerge(backend),
				Layer.provideMerge(JournalNotifierLive),
				Layer.provideMerge(database),
			);

			return yield* Effect.gen(function* () {
				const service = yield* SystemWakeLock;
				const notifier = yield* JournalNotifier;

				return yield* Script({
					events: Ref.get(events),
					SetSnapshot: (next) => Ref.set(snapshot, next),
					Signal: notifier.Publish(0),
					status: service.Status,
				});
			}).pipe(Effect.provide(wake_lock));
		}).pipe(Effect.provide(TestClock.layer())),
	);

describe("system wake lock", () => {
	it("acquires for unsettled work and releases after the linger", async () => {
		const events = await run_scenario(one_running, (context) =>
			Effect.gen(function* () {
				yield* TestClock.adjust(10);

				expect(yield* context.events).toEqual(["acquire"]);
				expect(yield* context.status).toEqual({ held: true, held_count: 1 });

				yield* context.SetSnapshot(idle);
				yield* context.Signal;
				yield* TestClock.adjust(poll_interval_ms);

				/** The count reached zero but the handle lingers. */
				expect(yield* context.events).toEqual(["acquire"]);

				yield* TestClock.adjust(linger_ms);

				expect(yield* context.status).toEqual({ held: false, held_count: 0 });

				return yield* context.events;
			}),
		);

		expect(events).toEqual(["acquire", "release"]);
	});

	it("does not thrash the OS handle while a queue drains item by item", async () => {
		const events = await run_scenario(one_running, (context) =>
			Effect.gen(function* () {
				yield* TestClock.adjust(10);
				yield* context.SetSnapshot(idle);
				yield* context.Signal;
				yield* TestClock.adjust(500);
				yield* context.SetSnapshot(one_running);
				yield* context.Signal;
				yield* TestClock.adjust(5_000);

				return yield* context.events;
			}),
		);

		expect(events).toEqual(["acquire"]);
	});

	it("holds an approval-blocked run for the grace period only", async () => {
		const events = await run_scenario(
			{ approval_requested_at_ms: [0], progressing_count: 0 },
			(context) =>
				Effect.gen(function* () {
					yield* TestClock.adjust(10);

					expect(yield* context.events).toEqual(["acquire"]);

					yield* TestClock.adjust(4_000);

					/** Still inside the grace window. */
					expect(yield* context.events).toEqual(["acquire"]);

					yield* TestClock.adjust(4_000);

					return yield* context.events;
				}),
		);

		expect(events).toEqual(["acquire", "release"]);
	});

	it("degrades to a no-op when the platform refuses the assertion", async () => {
		const status = await run_scenario(
			one_running,
			(context) =>
				Effect.gen(function* () {
					yield* TestClock.adjust(3_000);

					return yield* context.status;
				}),
			{ unavailable: true },
		);

		expect(status).toEqual({ held: false, held_count: 1 });
	});
});

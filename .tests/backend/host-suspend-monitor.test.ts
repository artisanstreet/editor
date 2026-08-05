import { describe, expect, it } from "vitest";
import { Effect, PubSub, Ref } from "effect";
import { TestClock } from "effect/testing";

import {
	HostSuspendMonitor,
	MakeHostSuspendMonitorLayer,
	type HostResume,
} from "../../modules/backend/src/host/suspend-monitor";

const heartbeat_ms = 1_000;
const minimum_gap_ms = 30_000;

/**
 * Drives the monitor across a scripted wall clock while the test clock advances
 * one heartbeat per entry. Suspends are exactly what the two clocks disagree
 * about, so a script entry larger than the previous one plus a heartbeat is a
 * suspend of the difference.
 */
const observe = (wall_clock_script: ReadonlyArray<number>) =>
	Effect.runPromise(
		Effect.gen(function* () {
			const wall_clock = yield* Ref.make(0);

			return yield* Effect.gen(function* () {
				const monitor = yield* HostSuspendMonitor;
				const subscription = yield* monitor.Subscribe;
				const seen = yield* Ref.make<ReadonlyArray<HostResume>>([]);

				yield* Effect.forkScoped(
					PubSub.take(subscription).pipe(
						Effect.flatMap((resume) =>
							Ref.update(seen, (current) => [...current, resume]),
						),
						Effect.forever,
					),
				);

				for (const wall_clock_ms of wall_clock_script) {
					yield* Ref.set(wall_clock, wall_clock_ms);
					yield* TestClock.adjust(heartbeat_ms);
				}

				return yield* Ref.get(seen);
			}).pipe(
				Effect.provide(
					MakeHostSuspendMonitorLayer({
						heartbeat_ms,
						minimum_gap_ms,
						Now: Ref.get(wall_clock),
					}),
				),
				Effect.scoped,
			);
		}).pipe(Effect.provide(TestClock.layer())),
	);

describe("host suspend monitor", () => {
	it("stays quiet while both clocks agree", async () => {
		const resumes = await observe([1_000, 2_000, 3_000, 4_000]);

		expect(resumes).toEqual([]);
	});

	it("reports the time the host was away", async () => {
		/** The final tick charges ten minutes of wall clock to one heartbeat. */
		const resumes = await observe([1_000, 2_000, 3_000, 604_000]);

		expect(resumes).toEqual([{ resumed_at_ms: 604_000, suspended_ms: 600_000 }]);
	});

	/** Scheduler drift and small clock corrections are not suspends. */
	it("ignores a gap below the reporting threshold", async () => {
		const resumes = await observe([1_000, 2_000, 3_000, 14_000]);

		expect(resumes).toEqual([]);
	});

	it("keeps watching after a suspend", async () => {
		const resumes = await observe([1_000, 604_000, 605_000, 1_206_000]);

		expect(resumes.map((resume) => resume.suspended_ms)).toEqual([602_000, 600_000]);
	});
});

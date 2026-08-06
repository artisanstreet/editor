import { Effect, Stream } from "effect";
import { describe, expect, it } from "vitest";

import {
	describe_diagnostic_cause,
	make_client_diagnostics,
} from "../../../modules/transport/src/internal/client-diagnostics";

/** Deterministic runtime: each observation advances the clock by one second. */
const make_fixed_runtime = () => {
	let tick = 0;

	return {
		MakeId: (prefix: string) =>
			Effect.sync(() => {
				tick += 1;

				return `${prefix}_${tick}`;
			}),
		Now: Effect.sync(() => {
			tick += 1;

			return new Date(1_754_000_000_000 + tick * 1_000).toISOString();
		}),
	};
};

describe("client diagnostics journal", () => {
	it("stamps recorded events and keeps them oldest first", async () => {
		const snapshot = await Effect.runPromise(
			Effect.gen(function* () {
				const diagnostics = yield* make_client_diagnostics(8, make_fixed_runtime());

				yield* diagnostics.Record({ kind: "session.attempt", ordinal: 1 });
				yield* diagnostics.Record({ kind: "supervisor.retry_released" });

				return yield* diagnostics.Snapshot;
			}),
		);

		expect(snapshot.dropped).toBe(0);
		expect(snapshot.events.map((event) => event.kind)).toEqual([
			"session.attempt",
			"supervisor.retry_released",
		]);
		const [first, second] = snapshot.events;
		expect(first !== undefined && second !== undefined && first.at < second.at).toBe(true);
	});

	it("evicts the oldest events beyond capacity and counts them", async () => {
		const snapshot = await Effect.runPromise(
			Effect.gen(function* () {
				const diagnostics = yield* make_client_diagnostics(2, make_fixed_runtime());

				yield* diagnostics.Record({ kind: "session.attempt", ordinal: 1 });
				yield* diagnostics.Record({ kind: "session.attempt", ordinal: 2 });
				yield* diagnostics.Record({ kind: "session.attempt", ordinal: 3 });

				return yield* diagnostics.Snapshot;
			}),
		);

		expect(snapshot.dropped).toBe(1);
		expect(
			snapshot.events.map((event) => (event.kind === "session.attempt" ? event.ordinal : -1)),
		).toEqual([2, 3]);
	});

	it("feeds live observers every recorded event", async () => {
		const events = await Effect.runPromise(
			Effect.gen(function* () {
				const diagnostics = yield* make_client_diagnostics(8, make_fixed_runtime());

				yield* diagnostics.Record({ kind: "session.attempt", ordinal: 1 });
				yield* diagnostics.Record({ kind: "client.disposed" });

				return [...(yield* diagnostics.Changes.pipe(Stream.take(2), Stream.runCollect))];
			}),
		);

		expect(events.map((event) => event.kind)).toEqual(["session.attempt", "client.disposed"]);
	});

	it("renders unknown causes as short human-readable detail", () => {
		expect(describe_diagnostic_cause(new Error("socket closed"))).toBe("socket closed");
		expect(describe_diagnostic_cause("stale transport close")).toBe("stale transport close");
		expect(describe_diagnostic_cause({ kind: "transport.close", reason: "restart" })).toBe(
			'{"kind":"transport.close","reason":"restart"}',
		);
		expect(describe_diagnostic_cause({ payload: "x".repeat(400) }).length).toBe(240);

		const cyclic: { self?: unknown } = {};
		cyclic.self = cyclic;
		expect(describe_diagnostic_cause(cyclic)).toBe("[object Object]");
	});
});

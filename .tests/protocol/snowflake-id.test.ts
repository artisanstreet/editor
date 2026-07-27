import { Effect, Schema } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";

import {
	Identifier,
	MakeSnowflakeIdLive,
	SnowflakeEpochMilliseconds,
	SnowflakeId,
} from "@artisan/protocol";

const DecodePayload = (identifier: string) => BigInt(identifier.slice(identifier.indexOf("_") + 1));

const RunGenerator = <A>(effect: Effect.Effect<A, never, SnowflakeId>) =>
	Effect.runPromise(
		effect.pipe(Effect.provide(MakeSnowflakeIdLive(37)), Effect.provide(TestClock.layer())),
	);

describe("SnowflakeId", () => {
	it("uses the exact 19 June 2026 UTC epoch and preserves identifier prefixes", async () => {
		const identifier = await RunGenerator(
			Effect.gen(function* () {
				yield* TestClock.setTime(SnowflakeEpochMilliseconds);

				return yield* (yield* SnowflakeId).Make("event");
			}),
		);

		expect(identifier).toBe(`event_${37n << 12n}`);
		expect(Schema.is(Identifier)(identifier)).toBe(true);
	});

	it("allocates bare identifiers for domains whose public identity is the snowflake", async () => {
		const identifier = await RunGenerator(
			Effect.gen(function* () {
				yield* TestClock.setTime(SnowflakeEpochMilliseconds);

				return yield* (yield* SnowflakeId).MakeBare;
			}),
		);

		expect(identifier).toBe((37n << 12n).toString(10));
		expect(identifier).toMatch(/^\d+$/);
		expect(Schema.is(Identifier)(identifier)).toBe(true);
	});

	it("allocates unique ordered sequences atomically under concurrency", async () => {
		const identifiers = await RunGenerator(
			Effect.gen(function* () {
				yield* TestClock.setTime(SnowflakeEpochMilliseconds + 10);
				const generator = yield* SnowflakeId;

				return yield* Effect.forEach(
					Array.from({ length: 1_000 }),
					() => generator.Make("message"),
					{ concurrency: "unbounded" },
				);
			}),
		);
		const payloads = identifiers.map(DecodePayload);

		expect(new Set(payloads).size).toBe(1_000);
		expect([...payloads].sort((left, right) => (left < right ? -1 : 1))).toEqual(
			Array.from(
				{ length: 1_000 },
				(_, sequence) => (10n << 22n) | (37n << 12n) | BigInt(sequence),
			),
		);
	});

	it("remains monotonic across clock rollback and sequence overflow", async () => {
		const payloads = await RunGenerator(
			Effect.gen(function* () {
				yield* TestClock.setTime(SnowflakeEpochMilliseconds + 100);
				const generator = yield* SnowflakeId;
				const first = yield* generator.Make("run");

				yield* TestClock.setTime(SnowflakeEpochMilliseconds + 50);
				const after_rollback = yield* generator.Make("run");
				const overflow = yield* Effect.forEach(Array.from({ length: 4_095 }), () =>
					generator.Make("run"),
				);

				return [first, after_rollback, ...overflow].map(DecodePayload);
			}),
		);

		expect(
			payloads.every((payload, index) => index === 0 || payload > payloads[index - 1]!),
		).toBe(true);
		expect(payloads.at(-1)! >> 22n).toBe(101n);
	});
});

import { ChildProcess } from "effect/unstable/process";
import { Deferred, Effect, Stream } from "effect";
import { describe, expect, it } from "vitest";

import { NormalizeConfiguration } from "../src/configuration.ts";
import { ReadinessError } from "../src/error.ts";
import { AwaitOutputReadiness, DecodeOutput, MatchesOutputReadiness } from "../src/readiness.ts";

const Encoder = new TextEncoder();

describe("runner readiness", () => {
	it("splits readiness output across byte chunks", async () => {
		const lines = await Effect.runPromise(
			DecodeOutput(
				"stdout",
				Stream.make(Encoder.encode("Forge rea"), Encoder.encode("dy\nnext\n")),
			).pipe(Stream.runCollect),
		);

		expect([...lines]).toEqual([
			{ line: "Forge ready", stream: "stdout" },
			{ line: "next", stream: "stdout" },
		]);
	});

	it("does not retain global regexp state between output lines", () => {
		const readiness = {
			_tag: "Output" as const,
			pattern: /ready/gu,
			stream: "either" as const,
			timeout: 1_000,
		};
		expect(MatchesOutputReadiness(readiness, { line: "ready", stream: "stdout" })).toBe(true);
		expect(MatchesOutputReadiness(readiness, { line: "ready", stream: "stderr" })).toBe(true);
	});

	it("fails output readiness with a typed timeout", async () => {
		const result = await Effect.runPromiseExit(
			Effect.gen(function* () {
				const ready = yield* Deferred.make<void, ReadinessError>();
				return yield* AwaitOutputReadiness(
					"forge",
					{ _tag: "Output", pattern: /ready/u, stream: "either", timeout: 1 },
					ready,
				);
			}),
		);
		expect(result._tag).toBe("Failure");
	});

	it("normalizes defaults while retaining structured ChildProcess commands", async () => {
		const command = ChildProcess.make`pnpm run dev`;
		const configuration = await Effect.runPromise(
			NormalizeConfiguration([{ command, name: "Forge" }]),
		);
		expect(configuration.processes[0]?.command).toBe(command);
		expect(configuration.processes[0]?.readiness).toEqual({ _tag: "Immediate" });
	});

	it("rejects duplicate dashboard lane ids", async () => {
		const result = await Effect.runPromiseExit(
			NormalizeConfiguration([{ command: ChildProcess.make`pnpm run dev`, name: "Forge" }], {
				lanes: [
					{ id: "forge", name: "Forge" },
					{ id: "forge", name: "Duplicate" },
				],
			}),
		);
		expect(result._tag).toBe("Failure");
	});
});

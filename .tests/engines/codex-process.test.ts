import { Buffer } from "node:buffer";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";
import { Effect, Fiber } from "effect";

import {
	CodexProcessFactory,
	CodexProcessFactoryLive,
	type CodexProcessHandle,
} from "@artisan/engines";

const fixture_path = fileURLToPath(new URL("./fixtures/fake-child.mjs", import.meta.url));
const encoder = new TextEncoder();
const decoder = new TextDecoder();

function make_scenario(
	chunks: ReadonlyArray<unknown>,
	exit?: { readonly at_ms: number; readonly code: number },
) {
	return encoder.encode(`${JSON.stringify({ chunks, ...(exit ? { exit } : {}) })}\n`);
}

function read_stdout(handle: CodexProcessHandle) {
	return Effect.tryPromise({
		try: async () => {
			const chunks: Array<Uint8Array> = [];

			for await (const chunk of handle.Stdout) {
				chunks.push(chunk);
			}

			return decoder.decode(Buffer.concat(chunks));
		},
		catch: (cause) => cause,
	});
}

function spawn_fixture() {
	return Effect.gen(function* () {
		const factory = yield* CodexProcessFactory;

		return yield* factory.Spawn({
			args: [fixture_path],
			command: process.execPath,
		});
	});
}

describe("Codex process factory", () => {
	it("starts a real fixture, transfers exact chunks, and observes exit", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const handle = yield* spawn_fixture();
				const output_fiber = yield* Effect.forkChild(read_stdout(handle));

				yield* handle.Write(
					make_scenario(
						[
							{ at_ms: 0, chunk_base64: "aGVs" },
							{ at_ms: 1, chunk_base64: "bG8K" },
						],
						{ at_ms: 5, code: 7 },
					),
				);

				return {
					exit: yield* handle.Exit,
					output: yield* Fiber.join(output_fiber),
				};
			}).pipe(Effect.provide(CodexProcessFactoryLive)),
		);

		expect(result.output).toBe("hello\n");
		expect(result.exit).toMatchObject({ code: 7, signal: null });
	});

	it("cancels an active child process through its handle", async () => {
		const exit = await Effect.runPromise(
			Effect.gen(function* () {
				const handle = yield* spawn_fixture();

				yield* handle.Write(make_scenario([]));
				yield* handle.Kill();

				return yield* handle.Exit;
			}).pipe(Effect.provide(CodexProcessFactoryLive)),
		);

		expect(exit.code === 143 || exit.signal === "SIGTERM").toBe(true);
	});

	it("cleans up a live child process when its handle closes", async () => {
		const exit = await Effect.runPromise(
			Effect.gen(function* () {
				const handle = yield* spawn_fixture();

				yield* handle.Write(make_scenario([]));
				yield* handle.Close;

				return yield* handle.Exit;
			}).pipe(Effect.provide(CodexProcessFactoryLive)),
		);

		expect(exit.code === 143 || exit.signal === "SIGTERM").toBe(true);
	});
});

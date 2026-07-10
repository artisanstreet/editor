import { readFile } from "node:fs/promises";

import { describe, expect, it } from "vitest";
import { Cause, Effect, Exit, Fiber, Stream } from "effect";

import { CodexProcessFactory, open_codex_app_server_session } from "@artisan/engines";

import { make_transcript_replay } from "./harness/transcript-process";

const fixture_url = new URL("./fixtures/transcripts/app-server-handshake.json", import.meta.url);

function error_from(exit: Exit.Exit<unknown, unknown>) {
	return Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;
}

describe("Transcript process replay", () => {
	it("validates transcript data and replays a split UTF-8 app-server handshake", async () => {
		const source = await readFile(fixture_url, "utf8");
		const transcript = JSON.parse(source) as unknown;
		const replay = await Effect.runPromise(make_transcript_replay(transcript));
		const started_at_ms = Date.now();
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* open_codex_app_server_session({
						spawn: { args: [], command: "codex app-server" },
					});
					const initialized = yield* session.Handshake({
						client_name: "transcript-test",
						client_version: "0.3.0",
					});

					yield* replay.Assert;

					return initialized;
				}).pipe(Effect.provide(replay.Layer)),
			),
		);

		await Effect.runPromise(replay.AssertClosed);

		expect(result.result.userAgent).toBe(`transcript snowman: ${String.fromCodePoint(0x2603)}`);
		expect(Date.now() - started_at_ms).toBeGreaterThanOrEqual(35);
	});

	it("rejects invalid chunks, invocations, outbound frames, and configured backpressure", async () => {
		const invalid = await Effect.runPromiseExit(
			make_transcript_replay({
				args: [],
				chunks: [{ at_ms: -1, chunk_base64: "not base64", stream: "stdout" }],
				command: "codex",
				exit_code: 0,
				exit_signal: null,
			}),
		);
		const invalid_invocation = await Effect.runPromiseExit(
			make_transcript_replay({
				args: [],
				chunks: [],
				command: "",
				exit_code: 0,
				exit_signal: null,
			}),
		);
		const replay = await Effect.runPromise(
			make_transcript_replay({
				args: ["app-server"],
				chunks: [{ at_ms: 0, chunk_base64: "e30K", stream: "stdin" }],
				command: "codex",
				exit_code: null,
				exit_signal: null,
				fault: { _tag: "backpressure", write_capacity: 0 },
			}),
		);
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const factory = yield* CodexProcessFactory;
				const invocation = yield* factory
					.Spawn({ args: [], command: "wrong" })
					.pipe(Effect.exit);
				const handle = yield* factory.Spawn({ args: ["app-server"], command: "codex" });
				const write = yield* handle
					.Write(new TextEncoder().encode("{}\n"))
					.pipe(Effect.exit);

				return { invocation, write };
			}).pipe(Effect.provide(replay.Layer)),
		);

		expect(error_from(invalid)).toBeDefined();
		expect(error_from(invalid_invocation)).toBeDefined();
		expect(error_from(result.invocation)).toMatchObject({ operation: "spawn" });
		expect(error_from(result.write)).toMatchObject({ operation: "write" });
	});

	it("expresses crash, early EOF, malformed frames, and delayed request timeouts", async () => {
		const crash_replay = await Effect.runPromise(
			make_transcript_replay({
				args: [],
				chunks: [],
				command: "codex",
				exit_code: null,
				exit_signal: null,
				fault: { _tag: "crash", at_ms: 0, exit_code: 23 },
			}),
		);
		const eof_replay = await Effect.runPromise(
			make_transcript_replay({
				args: [],
				chunks: [],
				command: "codex",
				exit_code: null,
				exit_signal: null,
				fault: { _tag: "early_eof", at_ms: 0 },
			}),
		);
		const malformed_replay = await Effect.runPromise(
			make_transcript_replay({
				args: [],
				chunks: [
					{
						at_ms: 0,
						chunk_base64: "eyJpZCI6MSwibWV0aG9kIjoic2xvdyIsInBhcmFtcyI6e319Cg==",
						stream: "stdin",
					},
					{ at_ms: 0, chunk_base64: "bm90IGpzb24K", stream: "stdout" },
					{
						at_ms: 1,
						chunk_base64: "eyJpZCI6MSwicmVzdWx0Ijp7Im9rIjp0cnVlfX0K",
						stream: "stdout",
					},
				],
				command: "codex",
				exit_code: null,
				exit_signal: null,
				fault: { _tag: "early_eof", at_ms: 10 },
			}),
		);
		const timeout_replay = await Effect.runPromise(
			make_transcript_replay({
				args: [],
				chunks: [
					{
						at_ms: 0,
						chunk_base64: "eyJpZCI6MSwibWV0aG9kIjoic2xvdyIsInBhcmFtcyI6e319Cg==",
						stream: "stdin",
					},
				],
				command: "codex",
				exit_code: null,
				exit_signal: null,
				fault: { _tag: "early_eof", at_ms: 50 },
			}),
		);
		const crash_exit = await Effect.runPromise(
			Effect.gen(function* () {
				const factory = yield* CodexProcessFactory;
				const handle = yield* factory.Spawn({ args: [], command: "codex" });

				return yield* handle.Exit;
			}).pipe(Effect.provide(crash_replay.Layer)),
		);
		const eof_exit = await Effect.runPromise(
			Effect.gen(function* () {
				const factory = yield* CodexProcessFactory;
				const handle = yield* factory.Spawn({ args: [], command: "codex" });

				return yield* handle.Exit;
			}).pipe(Effect.provide(eof_replay.Layer)),
		);
		const diagnostics = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* open_codex_app_server_session({
						spawn: { args: [], command: "codex" },
					});
					const diagnostic_fiber = yield* session.Diagnostics.pipe(
						Stream.runCollect,
						Effect.forkChild,
					);

					const response = yield* session.Request("slow", {}, 100);

					yield* session.Close;

					return { diagnostics: yield* Fiber.join(diagnostic_fiber), response };
				}).pipe(Effect.provide(malformed_replay.Layer)),
			),
		);
		const timeout = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* open_codex_app_server_session({
						spawn: { args: [], command: "codex" },
					});

					return yield* session.Request("slow", {}, 1).pipe(Effect.exit);
				}).pipe(Effect.provide(timeout_replay.Layer)),
			),
		);

		expect(crash_exit).toEqual({ code: 23, signal: null });
		expect(eof_exit).toEqual({ code: 0, signal: null });
		expect(diagnostics.response.result).toEqual({ ok: true });
		expect(diagnostics.diagnostics).toEqual(
			expect.arrayContaining([expect.objectContaining({ level: "error", source: "stdout" })]),
		);
		expect(error_from(timeout)).toMatchObject({
			_tag: "CodexAppServerRequestTimeoutError",
			method: "slow",
			timeout_ms: 1,
		});
	});
});

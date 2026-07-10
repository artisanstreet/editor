import { describe, expect, it } from "vitest";
import { Effect, Exit, Fiber, Layer } from "effect";
import { TestClock } from "effect/testing";

import {
	CodexEngine,
	CodexProcessFactory,
	CodexProcessFactoryLive,
	CodexEngineDescriptor,
	make_codex_engine_layer,
	type CodexProcessHandle,
} from "@artisan/engines";

const encoder = new TextEncoder();
const snowman = String.fromCodePoint(0x2603);

function chunks_from_text(text: string, split_at: number) {
	const bytes = encoder.encode(text);

	return [bytes.subarray(0, split_at), bytes.subarray(split_at)];
}

function never_chunks(): AsyncIterable<Uint8Array> {
	return {
		[Symbol.asyncIterator]: () => ({
			next: () => new Promise<IteratorResult<Uint8Array>>(() => undefined),
		}),
	};
}

function make_handle(chunks: ReadonlyArray<Uint8Array>, writes: Array<string>): CodexProcessHandle {
	return {
		Close: Effect.void,
		Exit: Effect.succeed({ code: 0, signal: null }),
		Kill: () => Effect.void,
		Stderr: (async function* () {})(),
		Stdout: (async function* () {
			for (const chunk of chunks) {
				yield chunk;
			}
		})(),
		Write: (chunk) =>
			Effect.sync(() => {
				writes.push(new TextDecoder().decode(chunk));
			}),
	};
}

describe("Codex engine probe", () => {
	it("discovers the version and completes initialize through a fake process factory", async () => {
		const commands: Array<ReadonlyArray<string>> = [];
		const writes: Array<string> = [];
		const user_agent = `codex-cli/0.142.5 ${snowman}`;
		const initialize_response = `${JSON.stringify({
			id: 1,
			result: {
				codexHome: "C:\\Users\\Sander\\.codex",
				platformFamily: "windows",
				platformOs: "windows",
				userAgent: user_agent,
			},
		})}\n`;
		let spawn_count = 0;
		const process_layer = Layer.succeed(CodexProcessFactory, {
			Spawn: (input) =>
				Effect.sync(() => {
					commands.push(input.args);
					spawn_count += 1;

					return make_handle(
						spawn_count === 1
							? [encoder.encode("codex-cli 0.142.5\n")]
							: chunks_from_text(
									initialize_response,
									initialize_response.indexOf(snowman) + 1,
								),
						writes,
					);
				}),
		});

		const probe = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;

				return yield* engine.Probe({
					client_name: "artisan-conformance",
					client_version: "0.2.0",
				});
			}).pipe(Effect.provide(make_codex_engine_layer().pipe(Layer.provide(process_layer)))),
		);

		expect(probe.version).toBe("0.142.5");
		expect(probe.metadata.user_agent).toBe(user_agent);
		expect(probe.authentication.state).toBe("unknown");
		expect(probe.capabilities.probe.state).toBe("supported");

		expect(commands).toEqual([["--version"], ["app-server", "--stdio"]]);
		expect(JSON.parse(writes[0] ?? "{}")).toMatchObject({
			id: 1,
			method: "initialize",
			params: {
				clientInfo: { name: "artisan-conformance", version: "0.2.0" },
			},
		});
	});

	it("advertises Open honestly until the run slice exists", async () => {
		await expect(
			Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const engine = yield* CodexEngine;

						return yield* engine.Open({
							_tag: "start",
							artisan_run_id: "artisan-run-1",
							initial_text: "Start the task",
							working_directory: "C:\\workspace",
						});
					}).pipe(
						Effect.provide(
							make_codex_engine_layer().pipe(Layer.provide(CodexProcessFactoryLive)),
						),
					),
				),
			),
		).rejects.toMatchObject({ _tag: "EngineUnsupportedOperationError", operation: "open" });
		expect(CodexEngineDescriptor.capabilities.start).toMatchObject({ state: "unsupported" });
		expect(CodexEngineDescriptor.capabilities.events).toMatchObject({ state: "unsupported" });
	});

	const live_it = process.env.ARTISAN_ENGINE_LIVE === "1" ? it : it.skip;

	live_it("performs only the live version and initialize smoke check", async () => {
		const probe = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;

				return yield* engine.Probe({
					client_name: "artisan-engine-smoke",
					client_version: "0.2.0",
				});
			}).pipe(
				Effect.provide(
					make_codex_engine_layer().pipe(Layer.provide(CodexProcessFactoryLive)),
				),
			),
		);

		expect(probe.version).toBe("0.142.5");
	});

	it("times out a stalled initialize probe and closes the process", async () => {
		let close_count = 0;
		let spawn_count = 0;
		const process_layer = Layer.succeed(CodexProcessFactory, {
			Spawn: () =>
				Effect.sync(() => {
					spawn_count += 1;

					if (spawn_count === 1) {
						return make_handle([encoder.encode("codex-cli 0.142.5\n")], []);
					}

					return {
						Close: Effect.sync(() => {
							close_count += 1;
						}),
						Exit: Effect.never,
						Kill: () => Effect.void,
						Stderr: (async function* () {
							yield encoder.encode("diagnostic output");
						})(),
						Stdout: never_chunks(),
						Write: () => Effect.void,
					} satisfies CodexProcessHandle;
				}),
		});
		const exit = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;
				const probe_fiber = yield* engine
					.Probe({ client_name: "timeout-test", client_version: "0.2.0" })
					.pipe(Effect.forkChild);

				yield* TestClock.adjust(10);

				return yield* Fiber.await(probe_fiber);
			}).pipe(
				Effect.provide(TestClock.layer()),
				Effect.provide(
					make_codex_engine_layer({ initialize_timeout_ms: 10 }).pipe(
						Layer.provide(process_layer),
					),
				),
			),
		);

		expect(Exit.isFailure(exit)).toBe(true);
		expect(close_count).toBe(1);
	});
});

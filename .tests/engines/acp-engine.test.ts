import { describe, expect, it } from "@effect/vitest";
import { Deferred, Effect, Layer, Stream } from "effect";

import {
	CursorEngine,
	CursorAcpArgs,
	ClassifyCursorStartupFailure,
	EngineProcessFactory,
	GrokEngine,
	GrokAcpArgs,
	ResolveCursorModel,
	make_cursor_engine_layer,
	make_grok_engine_layer,
	type EngineProcessSpawnInput,
} from "@artisan/engines";

const bytes = (value: string) => new TextEncoder().encode(value);

class AsyncByteQueue implements AsyncIterable<Uint8Array> {
	readonly #chunks: Array<Uint8Array> = [];
	readonly #waiters: Array<(value: IteratorResult<Uint8Array>) => void> = [];
	#ended = false;

	Push(value: Uint8Array) {
		const waiter = this.#waiters.shift();
		if (waiter === undefined) this.#chunks.push(value);
		else waiter({ done: false, value });
	}

	End() {
		this.#ended = true;
		for (const waiter of this.#waiters.splice(0)) waiter({ done: true, value: undefined });
	}

	[Symbol.asyncIterator](): AsyncIterator<Uint8Array> {
		return {
			next: () => {
				const value = this.#chunks.shift();
				if (value !== undefined) return Promise.resolve({ done: false, value });
				if (this.#ended) return Promise.resolve({ done: true, value: undefined });
				return new Promise((resolve) => this.#waiters.push(resolve));
			},
		};
	}
}

const MakeFactory = (auth_method: string, startup_stderr?: string) => {
	const spawns: Array<EngineProcessSpawnInput> = [];
	const factory = EngineProcessFactory.of({
		Spawn: (input) =>
			Effect.gen(function* () {
				spawns.push(input);
				const stdout = new AsyncByteQueue();
				const stderr = new AsyncByteQueue();
				const exit = yield* Deferred.make<{
					code: number | null;
					signal: NodeJS.Signals | null;
				}>();
				let closed = false;
				let pending = "";
				const Push = (payload: unknown) =>
					stdout.Push(bytes(`${JSON.stringify(payload)}\n`));
				const CloseWith = (code: number) =>
					Effect.gen(function* () {
						if (closed) return;
						closed = true;
						stdout.End();
						stderr.End();
						yield* Deferred.succeed(exit, { code, signal: null });
					});
				const Close = Effect.gen(function* () {
					if (closed) return;
					yield* CloseWith(0);
				});
				return {
					Close,
					EndInput: Effect.void,
					Exit: Deferred.await(exit),
					Kill: () => Close,
					Stderr: stderr,
					Stdout: stdout,
					Write: (chunk: Uint8Array) =>
						Effect.gen(function* () {
							pending += new TextDecoder().decode(chunk);
							let newline = pending.indexOf("\n");
							while (newline !== -1) {
								const line = pending.slice(0, newline);
								pending = pending.slice(newline + 1);
								const message = JSON.parse(line) as {
									id?: number | string;
									method?: string;
									params?: Record<string, unknown>;
								};
								if (message.id !== undefined) {
									if (message.method === "initialize")
										Push({
											jsonrpc: "2.0",
											id: message.id,
											result: {
												agentCapabilities: { loadSession: true },
												authMethods: [
													{ id: auth_method, name: "Test login" },
												],
												protocolVersion: 1,
											},
										});
									else if (message.method === "authenticate") {
										if (startup_stderr !== undefined) {
											stderr.Push(bytes(startup_stderr));
											yield* CloseWith(1);
											return;
										}
										Push({ jsonrpc: "2.0", id: message.id, result: {} });
									} else if (message.method === "session/new")
										Push({
											jsonrpc: "2.0",
											id: message.id,
											result: { sessionId: "native-session" },
										});
									else if (message.method === "session/prompt") {
										const sessionId = message.params?.sessionId;
										Push({
											jsonrpc: "2.0",
											method: "session/update",
											params: {
												sessionId,
												update: {
													content: { text: "Done", type: "text" },
													messageId: "assistant-1",
													sessionUpdate: "agent_message_chunk",
												},
											},
										});
										Push({
											jsonrpc: "2.0",
											method: "session/update",
											params: {
												sessionId,
												update: {
													kind: "execute",
													rawInput: { command: "pnpm test" },
													sessionUpdate: "tool_call",
													status: "pending",
													title: "Run tests",
													toolCallId: "tool-1",
												},
											},
										});
										Push({
											jsonrpc: "2.0",
											method: "session/update",
											params: {
												sessionId,
												update: {
													kind: "execute",
													rawOutput: { exitCode: 0, output: "passed" },
													sessionUpdate: "tool_call_update",
													status: "completed",
													toolCallId: "tool-1",
												},
											},
										});
										Push({
											jsonrpc: "2.0",
											method: "session/update",
											params: {
												sessionId,
												update: {
													sessionUpdate: "usage_update",
													size: 200_000,
													used: 42,
												},
											},
										});
										Push({
											jsonrpc: "2.0",
											id: message.id,
											result: {
												stopReason: "end_turn",
												usage: {
													inputTokens: 10,
													outputTokens: 5,
													totalTokens: 15,
												},
											},
										});
									}
								}
								newline = pending.indexOf("\n");
							}
						}),
				};
			}),
	});
	return { factory, spawns };
};

const OpenInput = {
	_tag: "start" as const,
	artisan_run_id: "artisan-run",
	initial_text: "Implement it",
	permission_policy: {
		approval: "never" as const,
		edit_scope: "host" as const,
		network_access: true,
		write_access: true,
	},
	working_directory: process.cwd(),
};

describe("ACP engines", () => {
	it("maps the unified Read only and Auto levels to native ACP modes", () => {
		expect(
			GrokAcpArgs({
				...OpenInput,
				provider_options: { "grok.permission_mode": "auto" },
			}),
		).toEqual(["--no-auto-update", "--permission-mode", "auto", "agent", "stdio"]);
		expect(
			GrokAcpArgs({
				...OpenInput,
				permission_policy: {
					approval: "on_request",
					network_access: false,
					write_access: false,
				},
			}),
		).toEqual(["--no-auto-update", "--permission-mode", "plan", "agent", "stdio"]);
		expect(
			CursorAcpArgs({
				...OpenInput,
				permission_policy: {
					approval: "never",
					network_access: false,
					write_access: false,
				},
				provider_options: { "cursor.permission_mode": "ask" },
			}),
		).toEqual(["--mode", "ask", "acp"]);
		expect(
			CursorAcpArgs({
				...OpenInput,
				permission_policy: {
					approval: "on_request",
					edit_scope: "host",
					network_access: true,
					write_access: true,
				},
				provider_options: { "cursor.permission_mode": "default" },
			}),
		).toEqual(["acp"]);
	});

	it.effect("runs Grok ACP end to end and translates its launch policy", () => {
		const fixture = MakeFactory("cached_token");
		const layer = make_grok_engine_layer({ executable: "fake-grok" }).pipe(
			Layer.provide(Layer.succeed(EngineProcessFactory, fixture.factory)),
		);

		return Effect.scoped(
			Effect.gen(function* () {
				const engine = yield* GrokEngine;
				const run = yield* engine.Open({
					...OpenInput,
					model: "grok-4.6",
					provider_options: {
						"grok.permission_mode": "auto",
						"grok.reasoning_effort": "xhigh",
					},
				});
				const observations = yield* Stream.runCollect(run.Events);

				expect(run.native_thread_id).toBe("native-session");
				expect(observations.map((item) => item._tag)).toContain("agent_message_completed");
				expect(observations.map((item) => item._tag)).toContain("terminal_activity");
				expect(observations.at(-1)).toMatchObject({
					_tag: "run_terminal",
					state: "completed",
				});
				expect(fixture.spawns[0]?.args).toEqual([
					"--no-auto-update",
					"--model",
					"grok-4.6",
					"--reasoning-effort",
					"xhigh",
					"--permission-mode",
					"auto",
					"agent",
					"stdio",
				]);
			}).pipe(Effect.provide(layer)),
		);
	});

	it.effect("runs Cursor ACP with Cursor's exact flattened model id and force mode", () => {
		const fixture = MakeFactory("cursor_login");
		const layer = make_cursor_engine_layer({ executable: "fake-cursor" }).pipe(
			Layer.provide(Layer.succeed(EngineProcessFactory, fixture.factory)),
		);

		return Effect.scoped(
			Effect.gen(function* () {
				const engine = yield* CursorEngine;
				const run = yield* engine.Open({
					...OpenInput,
					model: "cursor-grok-4.6",
					provider_options: {
						"cursor.permission_mode": "force",
						"cursor.reasoning_effort": "high",
						"cursor.speed": "fast",
					},
				});
				const observations = yield* Stream.runCollect(run.Events);

				expect(observations).toContainEqual(
					expect.objectContaining({ _tag: "agent_message_completed", message: "Done" }),
				);
				expect(fixture.spawns[0]?.args).toEqual([
					"--model",
					"cursor-grok-4.6-high-fast",
					"--force",
					"acp",
				]);
			}).pipe(Effect.provide(layer)),
		);
	});

	it("does not duplicate variants on an account-discovered Cursor model id", () => {
		expect(
			ResolveCursorModel({
				...OpenInput,
				model: "cursor-grok-4.6-high-fast",
				provider_options: {
					"cursor.reasoning_effort": "high",
					"cursor.speed": "fast",
				},
			}),
		).toBe("cursor-grok-4.6-high-fast");
	});

	it("classifies Cursor's invalid-model stderr without exposing generic ACP failure", () => {
		expect(
			ClassifyCursorStartupFailure(
				"Cannot use this model: cursor-grok-4.6[effort=high,fast=true]. Valid models are: auto",
			),
		).toMatchObject({
			artisan_code: "AE-PROVIDER-206",
			engine_id: "cursor",
			_tag: "EngineUnavailableError",
		});
	});

	it.effect(
		"preserves Cursor's model classification when ACP closes during authentication",
		() => {
			const fixture = MakeFactory(
				"cursor_login",
				"Cannot use this model: removed-model. Valid models are: auto",
			);
			const layer = make_cursor_engine_layer({ executable: "fake-cursor" }).pipe(
				Layer.provide(Layer.succeed(EngineProcessFactory, fixture.factory)),
			);

			return Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CursorEngine;
					const failure = yield* engine
						.Open({ ...OpenInput, model: "removed-model" })
						.pipe(Effect.flip);
					expect(failure).toMatchObject({
						artisan_code: "AE-PROVIDER-206",
						engine_id: "cursor",
						_tag: "EngineUnavailableError",
					});
				}).pipe(Effect.provide(layer)),
			);
		},
	);
});

import { describe, expect, it } from "vitest";
import { Cause, Deferred, Effect, Exit, Fiber, Layer } from "effect";

import { McpTransport } from "../../modules/backend/src/marketplace/capabilities/mcp-transport";
import {
	EngineProcessStdioMcpDriverLive,
	make_stdio_mcp_transport_layer,
	StdioMcpDriver,
} from "../../modules/backend/src/marketplace/capabilities/stdio-transport";
import {
	EngineProcessError,
	EngineProcessFactory,
	type EngineProcessHandle,
} from "@artisan/engines";

const encoder = new TextEncoder();

function failure(exit: Exit.Exit<unknown, unknown>) {
	return Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;
}

function stream(chunks: ReadonlyArray<Uint8Array> = []): AsyncIterable<Uint8Array> {
	return {
		async *[Symbol.asyncIterator]() {
			yield* chunks;
		},
	};
}

function channel() {
	const values: Array<Uint8Array> = [];
	let wake: (() => void) | undefined;
	let done = false;
	return {
		push: (value: Uint8Array) => {
			values.push(value);
			wake?.();
		},
		close: () => {
			done = true;
			wake?.();
		},
		iterable: {
			async *[Symbol.asyncIterator]() {
				while (!done || values.length > 0) {
					if (values.length === 0)
						await new Promise<void>((resolve) => {
							wake = resolve;
						});
					const value = values.shift();
					if (value !== undefined) yield value;
				}
			},
		} satisfies AsyncIterable<Uint8Array>,
	};
}

function make_process(options: {
	readonly stderr?: ReadonlyArray<Uint8Array>;
	readonly stdout?: ReadonlyArray<Uint8Array>;
	readonly on_write?: (chunk: Uint8Array) => void;
	readonly write_failure_at?: number;
}) {
	let closed = false;
	let close_count = 0;
	let write_count = 0;
	const exit = Effect.runSync(
		Deferred.make<{ readonly code: number | null; readonly signal: null }>(),
	);
	const stdout = channel();
	const handle: EngineProcessHandle = {
		Close: Effect.sync(() => {
			closed = true;
			close_count += 1;
			stdout.close();
		}).pipe(Effect.andThen(Deferred.succeed(exit, { code: 0, signal: null })), Effect.asVoid),
		EndInput: Effect.void,
		Exit: Deferred.await(exit),
		Kill: () => Effect.void,
		Stderr: stream(options.stderr),
		Stdout: stdout.iterable,
		Write: (chunk) =>
			Effect.suspend(() => {
				write_count += 1;
				return write_count === options.write_failure_at
					? Effect.fail(new EngineProcessError({ cause: "fake", operation: "write" }))
					: Effect.sync(() => options.on_write?.(chunk));
			}),
	};
	for (const chunk of options.stdout ?? []) stdout.push(chunk);
	return {
		handle,
		stdout,
		crash: () => Effect.runPromise(Deferred.succeed(exit, { code: 1, signal: null })),
		close_count: () => close_count,
		is_closed: () => closed,
	};
}

function make_layer(
	process: EngineProcessHandle,
	spawned: Array<{ readonly args: ReadonlyArray<string>; readonly command: string }>,
) {
	return make_stdio_mcp_transport_layer({
		args: ["--literal;not-a-shell"],
		command: "fake-mcp",
		invocation_timeout_ms: 25,
		max_message_bytes: 256,
		max_pending_requests: 2,
		max_stderr_bytes: 8,
		startup_timeout_ms: 25,
	}).pipe(
		Layer.provide(EngineProcessStdioMcpDriverLive),
		Layer.provide(
			Layer.succeed(EngineProcessFactory, {
				Spawn: (input) =>
					Effect.sync(() => {
						spawned.push(input);
						return process;
					}),
			}),
		),
	);
}

describe("Marketplace stdio MCP transport", () => {
	it("does not spawn before Connect, preserves argv, and rejects a duplicate scoped session", async () => {
		const process = make_process({});
		const spawned: Array<{ readonly args: ReadonlyArray<string>; readonly command: string }> =
			[];
		const layer = make_layer(process.handle, spawned);
		await Effect.runPromise(Effect.void.pipe(Effect.provide(layer)));
		expect(spawned).toEqual([]);
		const duplicate = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const transport = yield* McpTransport;
					yield* transport.Connect();
					return yield* Effect.exit(transport.Connect());
				}).pipe(Effect.provide(layer)),
			),
		);
		expect(spawned).toEqual([{ args: ["--literal;not-a-shell"], command: "fake-mcp" }]);
		expect(failure(duplicate)).toMatchObject({
			_tag: "McpTransportError",
			operation: "start",
			state: "connected",
		});
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					return yield* (yield* McpTransport).Connect();
				}).pipe(Effect.provide(layer)),
			),
		);
		expect(spawned).toHaveLength(2);
	});

	it("routes fragmented and interleaved JSON-RPC responses and sends initialized", async () => {
		const writes: Array<string> = [];
		const responses = [
			'{"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":1}}\n{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-',
			'03-26","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"fake"}}}\n',
		];
		let process: ReturnType<typeof make_process>;
		process = make_process({
			on_write: (chunk) => {
				const write = new TextDecoder().decode(chunk);
				writes.push(write);
				const request = JSON.parse(write) as {
					readonly id?: number;
					readonly method?: string;
					readonly params?: { readonly cursor?: string };
				};
				if (request.id === undefined) return;
				if (request.method === "tools/list") {
					const result = request.params?.cursor
						? { tools: [{ name: "write", inputSchema: {} }] }
						: {
								nextCursor: "tools-2",
								tools: [
									{
										name: "read",
										title: "Read",
										description: "Reads",
										inputSchema: {},
										outputSchema: {},
										annotations: { readOnlyHint: true },
										icons: [],
									},
								],
							};
					process.stdout.push(
						encoder.encode(
							`${JSON.stringify({ jsonrpc: "2.0", id: request.id, result })}\n`,
						),
					);
				}
				if (request.method === "resources/list") {
					const result = request.params?.cursor
						? { resources: [{ uri: "file:///two", name: "two" }] }
						: {
								nextCursor: "resources-2",
								resources: [
									{
										uri: "file:///one",
										name: "one",
										title: "One",
										description: "First",
										mimeType: "text/plain",
										size: 1,
										annotations: { audience: ["user"] },
										icons: [],
									},
								],
							};
					process.stdout.push(
						encoder.encode(
							`${JSON.stringify({ jsonrpc: "2.0", id: request.id, result })}\n`,
						),
					);
				}
			},
		});
		const spawned: Array<{ readonly args: ReadonlyArray<string>; readonly command: string }> =
			[];
		const layer = make_layer(process.handle, spawned);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* (yield* McpTransport).Connect();
					const initialize = yield* session.Initialize.pipe(
						Effect.forkChild({ startImmediately: true }),
					);
					const tools = session.ListTools;
					const resources = session.ListResources;
					setTimeout(
						() =>
							responses.forEach((response) =>
								process.stdout.push(encoder.encode(response)),
							),
						0,
					);
					return yield* Effect.all([tools, resources, Fiber.join(initialize)], {
						concurrency: "unbounded",
					});
				}).pipe(Effect.provide(layer)),
			),
		);
		expect(result).toEqual([
			[
				{ name: "read", description: "Reads", input_schema: {} },
				{ name: "write", input_schema: {} },
			],
			[
				{ uri: "file:///one", name: "one", description: "First" },
				{ uri: "file:///two", name: "two" },
			],
			{ protocol_version: "2025-03-26", server_name: "fake" },
		]);
		expect(writes).toHaveLength(6);
		expect(writes[1]).toContain('"notifications/initialized"');
		expect(
			writes
				.slice(2)
				.every(
					(value) => value.includes('"tools/list"') || value.includes('"resources/list"'),
				),
		).toBe(true);
	});

	it("fails JSON-RPC errors, malformed and oversized output without exposing unchecked data", async () => {
		for (const [stdout, close_stdout] of [
			[['{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"nope"}}\n'], false],
			[["not json\n"], false],
			[[`${"x".repeat(300)}\n`], false],
			[['{"jsonrpc":"2.0","id":{"bad":true},"method":"sampling/createMessage"}\n'], false],
			[['{"jsonrpc":"2.0"'], true],
		] as const) {
			const process = make_process({});
			const spawned: Array<{
				readonly args: ReadonlyArray<string>;
				readonly command: string;
			}> = [];
			const exit = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session = yield* (yield* McpTransport).Connect();
						setTimeout(() => {
							stdout.forEach((value) => process.stdout.push(encoder.encode(value)));
							if (close_stdout) process.stdout.close();
						}, 0);
						return yield* Effect.exit(session.Initialize);
					}).pipe(Effect.provide(make_layer(process.handle, spawned))),
				),
			);
			expect(failure(exit)).toMatchObject({
				_tag: "McpTransportError",
				operation: "initialize",
			});
			expect(process.close_count()).toBe(1);
		}
	});

	it("enforces startup and invocation deadlines and removes timed-out waiters", async () => {
		for (const action of ["initialize", "call_tool"] as const) {
			let process: ReturnType<typeof make_process>;
			process = make_process({
				on_write: (chunk) => {
					if (
						action === "call_tool" &&
						new TextDecoder().decode(chunk).includes('"method":"initialize"')
					)
						process.stdout.push(
							encoder.encode(
								'{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake"}}}\n',
							),
						);
				},
			});
			const spawned: Array<{
				readonly args: ReadonlyArray<string>;
				readonly command: string;
			}> = [];
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session = yield* (yield* McpTransport).Connect();
						if (action === "call_tool") yield* session.Initialize;
						return yield* Effect.exit(
							action === "initialize"
								? session.Initialize
								: session.CallTool({ name: "wait", arguments: {} }),
						);
					}).pipe(Effect.provide(make_layer(process.handle, spawned))),
				),
			);
			expect(failure(result)).toMatchObject({ _tag: "McpTransportError", operation: action });
		}
	});

	it("rejects discovery immediately when Initialize was never begun", async () => {
		const writes: Array<string> = [];
		const process = make_process({
			on_write: (chunk) => writes.push(new TextDecoder().decode(chunk)),
		});
		const spawned: Array<{ readonly args: ReadonlyArray<string>; readonly command: string }> =
			[];
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* (yield* McpTransport).Connect();
					return yield* Effect.exit(session.ListTools);
				}).pipe(Effect.provide(make_layer(process.handle, spawned))),
			),
		);
		expect(failure(result)).toMatchObject({ operation: "list_tools", state: "unhealthy" });
		expect(writes).toEqual([]);
	});

	it("terminates unhealthy after a malformed discovery page", async () => {
		let process: ReturnType<typeof make_process>;
		process = make_process({
			on_write: (chunk) => {
				const request = JSON.parse(new TextDecoder().decode(chunk)) as {
					readonly id?: number;
					readonly method?: string;
				};
				if (request.method === "initialize")
					process.stdout.push(
						encoder.encode(
							`{"jsonrpc":"2.0","id":${request.id},"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake"}}}\n`,
						),
					);
				if (request.method === "tools/list")
					process.stdout.push(
						encoder.encode(
							`{"jsonrpc":"2.0","id":${request.id},"result":{"tools":[{"name":"bad","inputSchema":{},"unexpected":true}]}}\n`,
						),
					);
			},
		});
		const spawned: Array<{ readonly args: ReadonlyArray<string>; readonly command: string }> =
			[];
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* (yield* McpTransport).Connect();
					yield* session.Initialize;
					const listed = yield* Effect.exit(session.ListTools);
					const health = yield* session.Health;
					return { health, listed };
				}).pipe(Effect.provide(make_layer(process.handle, spawned))),
			),
		);
		expect(failure(result.listed)).toMatchObject({
			operation: "list_tools",
			state: "unhealthy",
		});
		expect(result.health).toBe("unhealthy");
		expect(process.close_count()).toBe(1);
	});

	it("poisons failed initialization, closes once, and rejects every later operation", async () => {
		for (const { process, response } of [
			{
				process: make_process({ write_failure_at: 2 }),
				response:
					'{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake"}}}\n',
			},
			{
				process: make_process({}),
				response:
					'{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":7,"capabilities":{},"serverInfo":{"name":"fake"}}}\n',
			},
		] as const) {
			const spawned: Array<{
				readonly args: ReadonlyArray<string>;
				readonly command: string;
			}> = [];
			setTimeout(() => process.stdout.push(encoder.encode(response)), 0);
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session = yield* (yield* McpTransport).Connect();
						const initialize = yield* Effect.exit(session.Initialize);
						const later = yield* Effect.exit(session.ListTools);
						return { initialize, later };
					}).pipe(Effect.provide(make_layer(process.handle, spawned))),
				),
			);
			expect(failure(result.initialize)).toMatchObject({ operation: "initialize" });
			expect(failure(result.later)).toMatchObject({ operation: "list_tools" });
			expect(process.close_count()).toBe(1);
		}
	});

	it("bounds pending requests after initialization without emitting the saturated request", async () => {
		let process: ReturnType<typeof make_process>;
		const writes: Array<string> = [];
		process = make_process({
			on_write: (chunk) => {
				const write = new TextDecoder().decode(chunk);
				writes.push(write);
				if (write.includes('"method":"initialize"'))
					process.stdout.push(
						encoder.encode(
							'{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake"}}}\n',
						),
					);
			},
		});
		const spawned: Array<{ readonly args: ReadonlyArray<string>; readonly command: string }> =
			[];
		const exits = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* (yield* McpTransport).Connect();
					yield* session.Initialize;
					setTimeout(() => {
						process.stdout.push(
							encoder.encode('{"jsonrpc":"2.0","id":2,"result":{"content":[]}}\n'),
						);
						process.stdout.push(
							encoder.encode('{"jsonrpc":"2.0","id":3,"result":{"content":[]}}\n'),
						);
					}, 5);
					return yield* Effect.all(
						["one", "two", "three"].map((name) =>
							Effect.exit(session.CallTool({ name, arguments: {} })),
						),
						{ concurrency: "unbounded" },
					);
				}).pipe(Effect.provide(make_layer(process.handle, spawned))),
			),
		);
		expect(exits.filter(Exit.isFailure)).toHaveLength(1);
		expect(writes.filter((write) => write.includes('"tools/call"'))).toHaveLength(2);
	});

	it("validates tool-call success, malformed results, and server-declared errors", async () => {
		for (const scenario of ["success", "malformed", "is_error"] as const) {
			let process: ReturnType<typeof make_process>;
			process = make_process({
				on_write: (chunk) => {
					const request = JSON.parse(new TextDecoder().decode(chunk)) as {
						readonly id?: number;
						readonly method?: string;
					};
					if (request.method === "initialize")
						process.stdout.push(
							encoder.encode(
								`{"jsonrpc":"2.0","id":${request.id},"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake"}}}\n`,
							),
						);
					if (request.method === "tools/call") {
						const result =
							scenario === "success"
								? {
										content: [
											{ type: "text", text: "ok", _meta: { source: "fake" } },
										],
										structuredContent: { value: 1 },
										isError: false,
										_meta: { trace: "one" },
									}
								: scenario === "is_error"
									? { content: [{ type: "text", text: "denied" }], isError: true }
									: { structuredContent: { invalid: true } };
						process.stdout.push(
							encoder.encode(
								`${JSON.stringify({ jsonrpc: "2.0", id: request.id, result })}\n`,
							),
						);
					}
				},
			});
			const spawned: Array<{
				readonly args: ReadonlyArray<string>;
				readonly command: string;
			}> = [];
			const outcome = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session = yield* (yield* McpTransport).Connect();
						yield* session.Initialize;
						const result = yield* Effect.exit(
							session.CallTool({ name: "run", arguments: {} }),
						);
						return {
							closed: process.is_closed(),
							health: yield* session.Health,
							result,
						};
					}).pipe(Effect.provide(make_layer(process.handle, spawned))),
				),
			);
			if (scenario === "success") {
				expect(Exit.isSuccess(outcome.result)).toBe(true);
				expect(outcome).toMatchObject({ closed: false, health: "connected" });
			} else {
				expect(failure(outcome.result)).toMatchObject({
					operation: "call_tool",
					state: scenario === "malformed" ? "unhealthy" : "connected",
				});
				expect(outcome).toMatchObject(
					scenario === "malformed"
						? { closed: true, health: "unhealthy" }
						: { closed: false, health: "connected" },
				);
			}
		}
	});

	it("releases an explicit session close for reconnect while rejecting a live duplicate", async () => {
		let opens = 0;
		const driver = Layer.succeed(StdioMcpDriver, {
			Open: () =>
				Effect.sync(() => {
					opens += 1;
					return {
						Initialize: Effect.succeed({ protocol_version: "1", server_name: "fake" }),
						ListTools: Effect.succeed([]),
						ListResources: Effect.succeed([]),
						CallTool: () => Effect.succeed({}),
						Health: Effect.succeed("connected" as const),
						Close: Effect.void,
					};
				}),
		});
		const layer = make_stdio_mcp_transport_layer({
			args: [],
			command: "fake",
			invocation_timeout_ms: 25,
			max_message_bytes: 256,
			max_pending_requests: 2,
			max_stderr_bytes: 8,
			startup_timeout_ms: 25,
		}).pipe(Layer.provide(driver));
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const transport = yield* McpTransport;
					const first = yield* transport.Connect();
					expect(Exit.isFailure(yield* Effect.exit(transport.Connect()))).toBe(true);
					yield* first.Close;
					yield* transport.Connect();
				}).pipe(Effect.provide(layer)),
			),
		);
		expect(opens).toBe(2);
	});

	it("enforces invocation timeout, crash and bounded stderr cleanup", async () => {
		for (const options of [{}, { stderr: [encoder.encode("too much stderr")] }] as const) {
			const process = make_process(options);
			const spawned: Array<{
				readonly args: ReadonlyArray<string>;
				readonly command: string;
			}> = [];
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session = yield* (yield* McpTransport).Connect();
						if (options.stderr === undefined)
							yield* Effect.promise(() => process.crash());
						return yield* Effect.exit(
							session.CallTool({ name: "wait", arguments: {} }),
						);
					}).pipe(Effect.provide(make_layer(process.handle, spawned))),
				),
			);
			expect(failure(result)).toMatchObject({
				_tag: "McpTransportError",
				operation: "call_tool",
			});
			expect(process.is_closed()).toBe(true);
		}
	});
});

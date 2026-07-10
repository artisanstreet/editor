import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Exit, Scope, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope, HelloEnvelope } from "@artisan/protocol";
import type {
	Engine,
	EngineCommand,
	EngineObservation,
	EngineOpenInput,
	EngineRun,
} from "@artisan/engines";
import { make_backend_runtime, ProtocolServer, type ProtocolConnection } from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const connection_scopes: Array<Scope.Scope> = [];

interface Instrumentation {
	readonly commands: Array<EngineCommand>;
	readonly events_consumed: () => number;
	readonly opened: () => number;
	readonly scopes_closed: () => number;
}

interface EngineOptions {
	readonly fail_open?: boolean;
	readonly fail_send?: boolean;
	readonly open_delay?: number;
}

function make_engine(options: EngineOptions = {}): {
	readonly engine: Engine;
	readonly instrumentation: Instrumentation;
} {
	let opened = 0;
	let events_consumed = 0;
	let scopes_closed = 0;
	const commands: Array<EngineCommand> = [];
	const capabilities = Object.fromEntries(
		[
			"approval",
			"auth",
			"cancel",
			"close",
			"events",
			"model_selection",
			"native_tools",
			"probe",
			"question",
			"raw_frames",
			"resume",
			"start",
			"steer",
			"subagents",
		].map((name) => [name, { state: "supported" as const }]),
	) as Engine["Descriptor"]["capabilities"];

	const Open = (input: EngineOpenInput) =>
		Effect.gen(function* () {
			if (options.open_delay) {
				yield* Effect.sleep(`${options.open_delay} millis`);
			}

			opened += 1;

			if (options.fail_open) {
				yield* Effect.fail({ _tag: "open_failed" } as never);
			}

			yield* Effect.addFinalizer(() => Effect.sync(() => void (scopes_closed += 1)));

			const observations: ReadonlyArray<EngineObservation> = [];
			const run: EngineRun = {
				artisan_run_id: input.artisan_run_id,
				Closed: Effect.never,
				Events: Stream.unwrap(
					Effect.sync(() => {
						events_consumed += 1;

						return Stream.concat(Stream.fromIterable(observations), Stream.never);
					}),
				),
				native_thread_id: `native:${input.artisan_run_id}`,
				resume_token: { native_thread_id: `native:${input.artisan_run_id}` },
				Send: (command) =>
					options.fail_send
						? Effect.fail({ _tag: "send_failed" } as never)
						: Effect.sync(() => void commands.push(command)),
			};

			return run;
		});

	return {
		engine: {
			Descriptor: {
				capabilities,
				display_name: "Instrumented lifecycle engine",
				id: "instrumented",
				transport: "test",
			},
			Open,
			Probe: () => Effect.die("Probe is not used by lifecycle tests"),
		} satisfies Engine,
		instrumentation: {
			commands,
			events_consumed: () => events_consumed,
			opened: () => opened,
			scopes_closed: () => scopes_closed,
		},
	};
}

function make_hello(): HelloEnvelope {
	return {
		kind: "hello",
		message_id: "hello_1",
		origin: "frontend",
		payload: { event_cursors: [], last_journal_sequence: 0, supported_protocol_versions: [1] },
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function make_command(message_id: string, payload: CommandEnvelope["payload"]): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		thread_id: "thread_1",
	};
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-orchestrator-lifecycle-"));
	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

async function open_connection(runtime: ReturnType<typeof make_backend_runtime>) {
	const connection_scope = await Effect.runPromise(Scope.make());

	connection_scopes.push(connection_scope);

	return runtime.runPromise(
		Effect.gen(function* () {
			const server = yield* ProtocolServer;
			const connection = yield* server.Open.pipe(Scope.provide(connection_scope));

			yield* connection.Receive(make_hello());
			yield* connection.Outbound.pipe(Stream.take(2), Stream.runDrain);
			yield* connection.Receive(
				make_command("create_thread", {
					title: "Lifecycle",
					type: "thread.create",
				}),
			);
			yield* connection.Outbound.pipe(Stream.take(2), Stream.runDrain);

			return connection;
		}),
	);
}

function wait_for_receipt(connection: ProtocolConnection, correlation_id: string) {
	return connection.Outbound.pipe(
		Stream.filter(
			(envelope) =>
				envelope.kind === "command.receipt" && envelope.correlation_id === correlation_id,
		),
		Stream.take(1),
		Stream.runCollect,
	);
}

afterEach(async () => {
	await Promise.all(
		connection_scopes
			.splice(0)
			.map((scope) => Effect.runPromise(Scope.close(scope, Exit.succeed(undefined)))),
	);
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("agent orchestrator lifecycle supervision", () => {
	it("returns the durable receipt before a slow engine open completes", async () => {
		const database_path = await make_database_path();
		const instrumented = make_engine({ open_delay: 100 });
		const runtime = make_backend_runtime({
			database_path,
			engines: [instrumented.engine],
			migrations_path,
		});

		try {
			const connection = await open_connection(runtime);
			const started_at = Date.now();
			const receipt = runtime.runPromise(
				Effect.gen(function* () {
					yield* connection.Receive(
						make_command("slow_open", {
							engine_id: "instrumented",
							text: "start",
							type: "thread.send_message",
							working_directory: "C:/work",
						}),
					);

					return yield* wait_for_receipt(connection, "slow_open");
				}),
			);

			expect(Date.now() - started_at).toBeLessThan(80);
			expect((await receipt)[0]).toMatchObject({ payload: { status: "accepted" } });
		} finally {
			await runtime.dispose();
		}
	});

	it("opens one claimed run when the same durable wakeup arrives concurrently", async () => {
		const database_path = await make_database_path();
		const instrumented = make_engine();
		const runtime = make_backend_runtime({
			database_path,
			engines: [instrumented.engine],
			migrations_path,
		});

		try {
			const connection = await open_connection(runtime);
			const command = make_command("duplicate_wakeup", {
				engine_id: "instrumented",
				text: "start",
				type: "thread.send_message",
				working_directory: "C:/work",
			});

			await Promise.all([
				runtime.runPromise(connection.Receive(command)),
				runtime.runPromise(connection.Receive(command)),
			]);
			await new Promise((resolve) => setTimeout(resolve, 50));

			expect(instrumented.instrumentation.opened()).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("closes a never-ending run scope when the runtime is disposed", async () => {
		const database_path = await make_database_path();
		const instrumented = make_engine();
		const runtime = make_backend_runtime({
			database_path,
			engines: [instrumented.engine],
			migrations_path,
		});

		const connection = await open_connection(runtime);
		await runtime.runPromise(
			connection.Receive(
				make_command("never_ending", {
					engine_id: "instrumented",
					text: "start",
					type: "thread.send_message",
					working_directory: "C:/work",
				}),
			),
		);
		await new Promise((resolve) => setTimeout(resolve, 30));

		await runtime.dispose();

		expect(instrumented.instrumentation.events_consumed()).toBe(1);
		expect(instrumented.instrumentation.scopes_closed()).toBe(1);
	});

	it("dispositions failed opens and sends so they are not retried", async () => {
		const database_path = await make_database_path();
		const failed_open = make_engine({ fail_open: true });
		const runtime = make_backend_runtime({
			database_path,
			engines: [failed_open.engine],
			migrations_path,
		});

		try {
			const connection = await open_connection(runtime);
			const failed_open_command = make_command("failed_open", {
				engine_id: "instrumented",
				text: "start",
				type: "thread.send_message",
				working_directory: "C:/work",
			});
			await Promise.all([
				runtime.runPromise(connection.Receive(failed_open_command)),
				runtime.runPromise(connection.Receive(failed_open_command)),
			]);
			await new Promise((resolve) => setTimeout(resolve, 50));

			expect(failed_open.instrumentation.opened()).toBe(1);
		} finally {
			await runtime.dispose();
		}

		const send_database_path = await make_database_path();
		const failed_send = make_engine({ fail_send: true });
		const send_runtime = make_backend_runtime({
			database_path: send_database_path,
			engines: [failed_send.engine],
			migrations_path,
		});

		try {
			const connection = await open_connection(send_runtime);
			await send_runtime.runPromise(
				connection.Receive(
					make_command("start_for_failed_send", {
						engine_id: "instrumented",
						text: "start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await new Promise((resolve) => setTimeout(resolve, 50));

			await send_runtime.runPromise(
				connection.Receive(make_command("failed_send", { type: "run.cancel" })),
			);
			await new Promise((resolve) => setTimeout(resolve, 50));

			expect(failed_send.instrumentation.commands).toHaveLength(0);
		} finally {
			await send_runtime.dispose();
		}
	});
});

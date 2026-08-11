import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Exit, Scope, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { HelloEnvelope } from "@artisan/protocol";
import type { AuthoritativeCommandEnvelope } from "../../modules/backend/src/persistence/orchestration/message-command";
import type {
	Engine,
	EngineCommand,
	EngineObservation,
	EngineOpenInput,
	EngineRun,
} from "@artisan/engines";
import {
	AgentOrchestrator,
	make_backend_runtime,
	ProtocolRouter,
	ProtocolServer,
} from "@artisan/backend";
import { ConversationReadModel } from "../../modules/backend/src/conversation";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const connection_scopes: Array<Scope.Scope> = [];

interface Instrumentation {
	readonly commands: Array<EngineCommand>;
	readonly events_consumed: () => number;
	readonly opened: () => number;
	readonly open_inputs: () => ReadonlyArray<EngineOpenInput>;
	readonly scopes_closed: () => number;
}

interface EngineOptions {
	readonly die_open_attempts?: number;
	/** Ends the run's event stream immediately without a terminal observation. */
	readonly end_without_terminal?: boolean;
	readonly fail_open?: boolean;
	readonly fail_resume?: boolean;
	readonly fail_send?: boolean;
	readonly open_delay?: number;
	readonly stall_resume?: boolean;
}

function make_engine(options: EngineOptions = {}): {
	readonly engine: Engine;
	readonly instrumentation: Instrumentation;
} {
	let opened = 0;
	let events_consumed = 0;
	let scopes_closed = 0;
	const commands: Array<EngineCommand> = [];
	const open_inputs: Array<EngineOpenInput> = [];
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
			open_inputs.push(input);
			if (options.stall_resume && input._tag === "resume") {
				yield* Effect.never;
			}

			if (opened <= (options.die_open_attempts ?? 0)) {
				yield* Effect.die("synthetic engine startup defect");
			}

			if (options.fail_open || (options.fail_resume && input._tag === "resume")) {
				yield* Effect.fail({ _tag: "open_failed" } as never);
			}

			const closed = yield* Deferred.make<"closed">();
			yield* Effect.addFinalizer(() =>
				Effect.gen(function* () {
					scopes_closed += 1;
					yield* Deferred.succeed(closed, "closed");
				}),
			);

			const observations: ReadonlyArray<EngineObservation> = [];
			const run: EngineRun = {
				artisan_run_id: input.artisan_run_id,
				Closed: options.end_without_terminal
					? Effect.succeed("closed" as const)
					: Deferred.await(closed),
				Events: Stream.unwrap(
					Effect.sync(() => {
						events_consumed += 1;

						return options.end_without_terminal
							? Stream.fromIterable(observations)
							: Stream.concat(
									Stream.fromIterable(observations),
									Stream.fromEffect(Deferred.await(closed)).pipe(Stream.drain),
								);
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
			open_inputs: () => open_inputs,
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

function make_command(
	message_id: string,
	payload: AuthoritativeCommandEnvelope["payload"],
): AuthoritativeCommandEnvelope {
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
			const orchestrator = yield* AgentOrchestrator;
			const router = yield* ProtocolRouter;
			const server = yield* ProtocolServer;
			const connection = yield* server.Open.pipe(Scope.provide(connection_scope));

			yield* connection.Receive(make_hello());
			yield* connection.Outbound.pipe(Stream.take(2), Stream.runDrain);
			yield* router.Route(
				make_command("create_thread", {
					title: "Lifecycle",
					type: "thread.create",
				}),
			);

			return {
				...connection,
				Receive: (envelope: AuthoritativeCommandEnvelope | HelloEnvelope) =>
					envelope.kind === "command" && envelope.payload.type === "thread.send_message"
						? orchestrator.Handle(envelope)
						: connection.Receive(envelope),
			};
		}),
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
				connection.Receive(
					make_command("slow_open", {
						engine_id: "instrumented",
						text: "start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);

			expect(Date.now() - started_at).toBeLessThan(80);
			expect(await receipt).toMatchObject({ status: "accepted" });
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

	it("terminalizes an engine startup defect and dispatches the next accepted message", async () => {
		const database_path = await make_database_path();
		const instrumented = make_engine({ die_open_attempts: 1 });
		const runtime = make_backend_runtime({
			database_path,
			engines: [instrumented.engine],
			migrations_path,
		});

		try {
			const connection = await open_connection(runtime);
			await runtime.runPromise(
				connection.Receive(
					make_command("defective_open", {
						engine_id: "instrumented",
						text: "first",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await expect
				.poll(() => instrumented.instrumentation.opened(), { timeout: 2_000 })
				.toBe(1);
			const failed_snapshot = await runtime.runPromise(
				Effect.gen(function* () {
					const conversations = yield* ConversationReadModel;

					return yield* conversations.ReadSnapshot("thread_1");
				}),
			);
			expect(failed_snapshot.status).toBe("available");
			if (failed_snapshot.status !== "available") {
				throw new Error("Expected a durable conversation snapshot");
			}
			expect(failed_snapshot.snapshot.turns.at(-1)?.lifecycle).toBe("failed");
			expect(failed_snapshot.snapshot.items).toContainEqual(
				expect.objectContaining({
					severity: "error",
					summary: "Engine startup failed before the native session became ready.",
					type: "native_event",
				}),
			);

			await runtime.runPromise(
				connection.Receive(
					make_command("after_defective_open", {
						engine_id: "instrumented",
						text: "second",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);

			await expect
				.poll(() => instrumented.instrumentation.opened(), { timeout: 2_000 })
				.toBe(2);
			await expect
				.poll(() => instrumented.instrumentation.events_consumed(), { timeout: 2_000 })
				.toBe(1);
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
		await expect
			.poll(() => instrumented.instrumentation.events_consumed(), { timeout: 2_000 })
			.toBe(1);

		await runtime.dispose();

		expect(instrumented.instrumentation.events_consumed()).toBe(1);
		expect(instrumented.instrumentation.scopes_closed()).toBe(1);
	});

	it("reopens a persisted native run once with Engine.Open resume after restart", async () => {
		const database_path = await make_database_path();
		const initial = make_engine();
		const initial_runtime = make_backend_runtime({
			database_path,
			engines: [initial.engine],
			migrations_path,
		});

		try {
			const connection = await open_connection(initial_runtime);
			await initial_runtime.runPromise(
				connection.Receive(
					make_command("resume_after_restart", {
						engine_id: "instrumented",
						text: "Continue this native thread",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await expect.poll(() => initial.instrumentation.opened(), { timeout: 2_000 }).toBe(1);
		} finally {
			await initial_runtime.dispose();
		}

		const recovery = make_engine();
		const recovery_runtime = make_backend_runtime({
			database_path,
			engines: [recovery.engine],
			migrations_path,
		});

		try {
			await recovery_runtime.runPromise(
				Effect.gen(function* () {
					yield* AgentOrchestrator;
				}),
			);
			await expect.poll(() => recovery.instrumentation.opened(), { timeout: 2_000 }).toBe(1);
			expect(recovery.instrumentation.open_inputs()).toEqual([
				expect.objectContaining({
					_tag: "resume",
					resume_token: { native_thread_id: expect.any(String) },
				}),
			]);
		} finally {
			await recovery_runtime.dispose();
		}
	});

	it.each([
		["is rejected", { fail_resume: true }],
		["defects", { die_open_attempts: 1 }],
	] as const)(
		"fails closed when a native resume %s and never starts a replacement run",
		async (_case, recovery_options) => {
			const database_path = await make_database_path();
			const initial = make_engine();
			const initial_runtime = make_backend_runtime({
				database_path,
				engines: [initial.engine],
				migrations_path,
			});

			try {
				const connection = await open_connection(initial_runtime);
				await initial_runtime.runPromise(
					connection.Receive(
						make_command("reject_resume", {
							engine_id: "instrumented",
							text: "Do not duplicate this run",
							type: "thread.send_message",
							working_directory: "C:/work",
						}),
					),
				);
				await expect
					.poll(() => initial.instrumentation.opened(), { timeout: 2_000 })
					.toBe(1);
			} finally {
				await initial_runtime.dispose();
			}

			const rejected = make_engine(recovery_options);
			const rejected_runtime = make_backend_runtime({
				database_path,
				engines: [rejected.engine],
				migrations_path,
			});

			try {
				await rejected_runtime.runPromise(
					Effect.gen(function* () {
						yield* AgentOrchestrator;
					}),
				);
				await expect
					.poll(() => rejected.instrumentation.opened(), { timeout: 2_000 })
					.toBe(1);
				expect(rejected.instrumentation.open_inputs()[0]).toMatchObject({ _tag: "resume" });
				const snapshot = await rejected_runtime.runPromise(
					Effect.gen(function* () {
						const conversations = yield* ConversationReadModel;

						return yield* conversations.ReadSnapshot("thread_1");
					}),
				);
				expect(snapshot.status).toBe("available");
				if (snapshot.status !== "available") {
					throw new Error("Expected a recovered conversation snapshot");
				}
				expect(snapshot.snapshot.turns.at(-1)?.lifecycle).toBe("failed");
			} finally {
				await rejected_runtime.dispose();
			}

			const later = make_engine();
			const later_runtime = make_backend_runtime({
				database_path,
				engines: [later.engine],
				migrations_path,
			});

			try {
				await later_runtime.runPromise(
					Effect.gen(function* () {
						yield* AgentOrchestrator;
					}),
				);
				await new Promise((resolve) => setTimeout(resolve, 50));
				expect(later.instrumentation.opened()).toBe(0);
			} finally {
				await later_runtime.dispose();
			}
		},
	);

	it("boots with a settled thread while native resume is stalled", async () => {
		const database_path = await make_database_path();
		const initial = make_engine();
		const initial_runtime = make_backend_runtime({
			database_path,
			engines: [initial.engine],
			migrations_path,
		});

		try {
			const connection = await open_connection(initial_runtime);
			await initial_runtime.runPromise(
				connection.Receive(
					make_command("stalled_resume", {
						engine_id: "instrumented",
						text: "Recover this native run without wedging Forge",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await expect.poll(() => initial.instrumentation.opened(), { timeout: 2_000 }).toBe(1);
		} finally {
			await initial_runtime.dispose();
		}

		const stalled = make_engine({ stall_resume: true });
		const recovery_runtime = make_backend_runtime({
			database_path,
			engines: [stalled.engine],
			migrations_path,
		});

		try {
			const started_at = Date.now();
			const snapshot = await recovery_runtime.runPromise(
				Effect.gen(function* () {
					yield* AgentOrchestrator;
					const conversations = yield* ConversationReadModel;

					return yield* conversations.ReadSnapshot("thread_1");
				}),
			);
			expect(Date.now() - started_at).toBeLessThan(2_000);
			expect(snapshot.status).toBe("available");
			if (snapshot.status !== "available") {
				throw new Error("Expected a recovered conversation snapshot");
			}
			expect(snapshot.snapshot.turns.at(-1)?.lifecycle).toBe("failed");
			await expect.poll(() => stalled.instrumentation.opened(), { timeout: 2_000 }).toBe(1);
		} finally {
			await recovery_runtime.dispose();
		}
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

	/**
	 * A run whose engine died without a terminal observation leaves the durable
	 * record claiming the run is still running while no live run exists to
	 * deliver commands to. The user's stop is the terminal for such a run: the
	 * cancel must finalize it as cancelled instead of dropping as undeliverable
	 * and leaving the run wedged forever.
	 */
	it("finalizes a cancel aimed at a run with no live engine", async () => {
		const database_path = await make_database_path();
		const vanished = make_engine({ end_without_terminal: true });
		const runtime = make_backend_runtime({
			database_path,
			engines: [vanished.engine],
			migrations_path,
		});

		try {
			const connection = await open_connection(runtime);
			await runtime.runPromise(
				connection.Receive(
					make_command("start_then_vanish", {
						engine_id: "instrumented",
						text: "start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await expect
				.poll(() => vanished.instrumentation.scopes_closed(), { timeout: 2_000 })
				.toBe(1);

			await runtime.runPromise(
				connection.Receive(make_command("stop_vanished", { type: "run.cancel" })),
			);

			const read_snapshot = () =>
				runtime.runPromise(
					Effect.gen(function* () {
						const conversations = yield* ConversationReadModel;

						return yield* conversations.ReadSnapshot("thread_1");
					}),
				);
			await expect
				.poll(
					async () => {
						const result = await read_snapshot();

						return result.status === "available"
							? result.snapshot.items.find((item) => item.type === "work_session")
									?.lifecycle
							: undefined;
					},
					{ timeout: 2_000 },
				)
				.toBe("cancelled");

			const result = await read_snapshot();
			if (result.status !== "available") {
				throw new Error("Expected a durable conversation snapshot");
			}
			const session = result.snapshot.items.find((item) => item.type === "work_session");
			expect(session).toMatchObject({ status: "cancelled" });
			expect(
				session !== undefined && "ended_at" in session ? session.ended_at : undefined,
			).toBeDefined();
			/** Nothing was alive to deliver to; the stop settled durably instead. */
			expect(vanished.instrumentation.commands).toHaveLength(0);
		} finally {
			await runtime.dispose();
		}
	});

	it("cancels live work and rejects new intake while draining for shutdown", async () => {
		const database_path = await make_database_path();
		const instrumented = make_engine();
		const runtime = make_backend_runtime({
			database_path,
			engines: [instrumented.engine],
			migrations_path,
		});

		try {
			const connection = await open_connection(runtime);
			await runtime.runPromise(
				connection.Receive(
					make_command("shutdown_active", {
						engine_id: "instrumented",
						text: "start",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			await expect.poll(instrumented.instrumentation.opened).toBe(1);

			const orchestrator = await runtime.runPromise(AgentOrchestrator);
			await runtime.runPromise(orchestrator.DrainForShutdown);

			expect(instrumented.instrumentation.commands).toContainEqual({
				_tag: "cancel",
				command_id: expect.stringMatching(/^shutdown:/),
			});
			expect(instrumented.instrumentation.scopes_closed()).toBe(1);
			await expect(
				runtime.runPromise(
					orchestrator.Handle(
						make_command("shutdown_rejected", {
							engine_id: "instrumented",
							text: "must not queue",
							type: "thread.send_message",
							working_directory: "C:/work",
						}),
					),
				),
			).rejects.toMatchObject({ _tag: "OrchestrationFailure" });
		} finally {
			await runtime.dispose();
		}
	});
});

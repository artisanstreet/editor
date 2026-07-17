import { mkdtemp, rm } from "node:fs/promises";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { Cause, Deferred, Effect, Fiber, Layer, Queue, Ref, Stream } from "effect";

import type {
	CommandEnvelope,
	HelloEnvelope,
	TerminalListQueryEnvelope,
	TerminalSession,
} from "@artisan/protocol";
import { DecodeCommandEnvelope } from "@artisan/protocol";
import {
	make_backend_runtime,
	ProtocolRouter,
	ProtocolServer,
	TerminalDriver,
	TerminalDriverError,
	TerminalNotFound,
	TerminalRepository,
	TerminalSessionService,
	type ProtocolConnection,
	type TerminalDriverExit,
} from "@artisan/backend";
import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { ThreadErasureClaims } from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const crash_fixture_path = fileURLToPath(new URL("./terminal-crash-fixture.ts", import.meta.url));
const typescript_loader_url = new URL("./terminal-typescript-loader.mjs", import.meta.url).href;
const temporary_directories: Array<string> = [];

interface FakeTerminal {
	readonly Finish: (exit: TerminalDriverExit) => Effect.Effect<void>;
	readonly close_release: Deferred.Deferred<void>;
	readonly close_started: Deferred.Deferred<void>;
	readonly exit: Deferred.Deferred<TerminalDriverExit>;
	readonly output: Queue.Queue<Uint8Array, Cause.Done<void>>;
	closed: boolean;
}

function make_fake_terminal_driver(
	options: {
		readonly block_close?: boolean;
		readonly exit_before_output_end?: boolean;
		readonly output_on_close?: Uint8Array | string;
		readonly output_capacity?: number;
	} = {},
) {
	const terminals: Array<FakeTerminal> = [];
	const output_capacity = options.output_capacity ?? 512;
	const stats = {
		clears: 0,
		closes: 0,
		kills: 0,
		opens: 0,
		resizes: [] as Array<{ readonly cols: number; readonly rows: number }>,
		scope_finalizers: 0,
		writes: [] as Array<string>,
	};
	const layer = Layer.succeed(TerminalDriver, {
		Open: (input) =>
			Effect.gen(function* () {
				let terminal: FakeTerminal;

				stats.opens += 1;
				yield* Effect.addFinalizer(() =>
					Effect.sync(() => {
						stats.scope_finalizers += 1;
					}),
				);

				if (input.executable === "spawn-failure") {
					return yield* new TerminalDriverError({
						cause: new Error("spawn failed"),
						operation: "spawn",
					});
				}

				const exit = yield* Deferred.make<TerminalDriverExit>();
				const output = yield* Queue.dropping<Uint8Array, Cause.Done<void>>(output_capacity);
				const output_ended = yield* Deferred.make<void>();
				const close_release = yield* Deferred.make<void>();
				const close_started = yield* Deferred.make<void>();

				const Finish = (terminal_exit: TerminalDriverExit) =>
					Effect.gen(function* () {
						if (terminal.closed) {
							return;
						}

						terminal.closed = true;
						yield* Queue.end(output);
						yield* Deferred.succeed(exit, terminal_exit);
					});

				terminal = {
					Finish,
					close_release,
					close_started,
					closed: false,
					exit,
					output,
				};
				terminals.push(terminal);

				const Close = Effect.gen(function* () {
					if (terminal.closed) {
						return;
					}

					stats.closes += 1;

					if (options.output_on_close !== undefined) {
						yield* Queue.offer(
							output,
							typeof options.output_on_close === "string"
								? new TextEncoder().encode(options.output_on_close)
								: Uint8Array.from(options.output_on_close),
						);
					}

					yield* Deferred.succeed(close_started, undefined);

					if (options.block_close) {
						yield* Deferred.await(close_release);
					}

					yield* Finish({ exit_code: null, reason: "closed", signal: null });
				});
				const Exit = options.exit_before_output_end
					? Deferred.await(exit)
					: Effect.gen(function* () {
							const terminal_exit = yield* Deferred.await(exit);

							yield* Deferred.await(output_ended);

							return terminal_exit;
						});
				const Output = Stream.fromQueue(output).pipe(
					Stream.ensuring(Deferred.succeed(output_ended, undefined)),
				);

				return {
					Clear: Effect.sync(() => {
						stats.clears += 1;
					}),
					Close,
					Exit,
					Kill: () =>
						Effect.gen(function* () {
							stats.kills += 1;
							yield* Finish({ exit_code: null, reason: "killed", signal: null });
						}),
					Output,
					Resize: (cols, rows) =>
						Effect.sync(() => {
							stats.resizes.push({ cols, rows });
						}),
					Write: (chunk) =>
						Effect.sync(() => {
							stats.writes.push(new TextDecoder().decode(chunk));
						}),
					pid: input.executable === "invalid-pid" ? Number.NaN : 10_000 + stats.opens,
				};
			}),
	});

	return {
		Emit: (index: number, chunk: Uint8Array | string) =>
			Effect.runPromise(
				Queue.offer(
					terminals[index]!.output,
					typeof chunk === "string"
						? new TextEncoder().encode(chunk)
						: Uint8Array.from(chunk),
				),
			),
		Exit: (index: number, exit: TerminalDriverExit) =>
			Effect.runPromise(terminals[index]!.Finish(exit)),
		ExitWithoutOutputEnd: (index: number, exit: TerminalDriverExit) =>
			Effect.runPromise(Deferred.succeed(terminals[index]!.exit, exit)),
		ReleaseClose: (index: number) =>
			Effect.runPromise(Deferred.succeed(terminals[index]!.close_release, undefined)),
		WaitForClose: (index: number) =>
			Effect.runPromise(Deferred.await(terminals[index]!.close_started)),
		layer,
		stats,
	};
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-editor-terminal-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function command(
	message_id: string,
	payload: CommandEnvelope["payload"],
	thread_id = "thread_terminal",
): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		thread_id,
	};
}

function open_command(
	message_id: string,
	terminal_id: string,
	executable = "fake-shell",
): CommandEnvelope {
	return command(message_id, {
		args: ["--interactive"],
		cols: 100,
		env: { "ProgramFiles(x86)": "C:\\Program Files (x86)", "A.B-C": "valid" },
		executable,
		rows: 30,
		terminal_id,
		type: "terminal.open",
		working_directory: tmpdir(),
		workspace_id: "workspace_1",
	});
}

function route(runtime: ReturnType<typeof make_backend_runtime>, input: CommandEnvelope) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const router = yield* ProtocolRouter;

			return yield* router.Route(input);
		}),
	);
}

function list(runtime: ReturnType<typeof make_backend_runtime>, thread_id = "thread_terminal") {
	return runtime.runPromise(
		Effect.gen(function* () {
			const terminals = yield* TerminalSessionService;

			return yield* terminals.List(thread_id, "workspace_1");
		}),
	);
}

function recent_output(
	runtime: ReturnType<typeof make_backend_runtime>,
	terminal_id: string,
	thread_id = "thread_terminal",
	workspace_id = "workspace_1",
	max_bytes = 65_536,
) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const terminals = yield* TerminalSessionService;

			return yield* terminals.RecentOutput(terminal_id, thread_id, workspace_id, max_bytes);
		}),
	);
}

function canonical(
	runtime: ReturnType<typeof make_backend_runtime>,
	input: CommandEnvelope,
	workspace_id: string,
) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const terminals = yield* TerminalSessionService;

			return yield* terminals.HandleCanonical(
				input as CommandEnvelope & {
					readonly payload: Extract<
						CommandEnvelope["payload"],
						{
							readonly type:
								| "terminal.open"
								| "terminal.write"
								| "terminal.resize"
								| "terminal.clear"
								| "terminal.kill"
								| "terminal.restart"
								| "terminal.close";
						}
					>;
				},
				workspace_id,
			);
		}),
	);
}

function claim_only(runtime: ReturnType<typeof make_backend_runtime>, input: CommandEnvelope) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const repository = yield* TerminalRepository;
			const decoded = yield* DecodeCommandEnvelope(input);

			return yield* repository.Claim(decoded, "fault_injection_instance");
		}),
	);
}

function create_thread(
	runtime: ReturnType<typeof make_backend_runtime>,
	thread_id = "thread_terminal",
) {
	return route(
		runtime,
		command(
			`create_${thread_id}`,
			{ title: `Terminal ${thread_id}`, type: "thread.create" },
			thread_id,
		),
	);
}

function lifecycle_event(output: Awaited<ReturnType<typeof route>>) {
	return output.find((envelope) => envelope.kind === "event");
}

async function wait_for_terminal(
	runtime: ReturnType<typeof make_backend_runtime>,
	predicate: (terminal: TerminalSession) => boolean,
) {
	for (let attempt = 0; attempt < 250; attempt += 1) {
		const [terminal] = await list(runtime);

		if (terminal && predicate(terminal)) {
			return terminal;
		}

		await new Promise<void>((resolve) => setTimeout(resolve, 10));
	}

	throw new Error("Terminal did not reach the expected state");
}

async function wait_for_recent_output(
	runtime: ReturnType<typeof make_backend_runtime>,
	terminal_id: string,
	predicate: (output: Uint8Array) => boolean,
) {
	for (let attempt = 0; attempt < 100; attempt += 1) {
		const recent = await recent_output(runtime, terminal_id);

		if (recent.state === "available" && predicate(recent.output)) {
			return recent;
		}

		await new Promise<void>((resolve) => setTimeout(resolve, 10));
	}

	throw new Error("Terminal recent output did not reach the expected state");
}

function make_hello(): HelloEnvelope {
	return {
		kind: "hello",
		message_id: "hello_terminal",
		origin: "frontend",
		payload: {
			event_cursors: [],
			last_journal_sequence: 0,
			supported_protocol_versions: [1],
		},
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function take(connection: ProtocolConnection, count: number) {
	return connection.Outbound.pipe(Stream.take(count), Stream.runCollect);
}

async function leave_crashed_terminal(database_path: string) {
	const child = spawn(
		process.execPath,
		["--experimental-loader", typescript_loader_url, crash_fixture_path],
		{
			cwd: process.cwd(),
			env: {
				...process.env,
				ARTISAN_TERMINAL_CRASH_DATABASE: database_path,
				ARTISAN_TERMINAL_CRASH_MIGRATIONS: migrations_path,
			},
			stdio: ["ignore", "pipe", "pipe"],
			windowsHide: true,
		},
	);
	let stderr = "";

	child.stderr.on("data", (chunk) => {
		stderr += String(chunk);
	});

	try {
		await new Promise<void>((resolve, reject) => {
			let stdout = "";
			const timeout = setTimeout(
				() => reject(new Error(`Crash fixture did not become ready: ${stderr}`)),
				10_000,
			);

			child.stdout.on("data", (chunk) => {
				stdout += String(chunk);

				if (stdout.includes("TERMINAL_CRASH_READY")) {
					clearTimeout(timeout);
					resolve();
				}
			});
			child.once("exit", (code) => {
				clearTimeout(timeout);
				reject(new Error(`Crash fixture exited early with ${code}: ${stderr}`));
			});
		});
	} finally {
		if (child.exitCode === null && child.signalCode === null) {
			child.kill();
		}

		if (child.exitCode === null && child.signalCode === null) {
			await new Promise<void>((resolve, reject) => {
				const timeout = setTimeout(
					() => reject(new Error("Crash fixture did not terminate")),
					10_000,
				);

				child.once("exit", () => {
					clearTimeout(timeout);
					resolve();
				});
			});
		}
	}
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("terminal session orchestration", () => {
	it("requires an asserted workspace on every existing-terminal mutation", async () => {
		const payloads = [
			{ data: "text", terminal_id: "terminal_1", type: "terminal.write" },
			{ cols: 100, rows: 30, terminal_id: "terminal_1", type: "terminal.resize" },
			{ terminal_id: "terminal_1", type: "terminal.clear" },
			{ terminal_id: "terminal_1", type: "terminal.kill" },
			{ terminal_id: "terminal_1", type: "terminal.close" },
			{ terminal_id: "terminal_1", type: "terminal.restart" },
		];
		const template = command("missing_workspace", { type: "run.cancel" });
		const exits = await Promise.all(
			payloads.map((payload, index) =>
				Effect.runPromise(
					DecodeCommandEnvelope({
						...template,
						message_id: `missing_workspace_${index}`,
						payload,
					}).pipe(Effect.exit),
				),
			),
		);

		expect(exits.every((exit) => exit._tag === "Failure")).toBe(true);
	});

	it("streams real node-pty input and records natural exit metadata", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			await create_thread(runtime);
			await route(
				runtime,
				command("open_real", {
					args: [
						"-e",
						`process.stdin.once("data", data => { process.stdout.write("ECHO:" + String(data).trim()); process.exit(9); });`,
					],
					cols: 100,
					executable: process.execPath,
					rows: 30,
					terminal_id: "terminal_real",
					type: "terminal.open",
					working_directory: tmpdir(),
					workspace_id: "workspace_1",
				}),
			);

			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const terminals = yield* TerminalSessionService;
						const stream = yield* terminals.Output(
							"terminal_real",
							"thread_terminal",
							"workspace_1",
						);
						const output_fiber = yield* stream.pipe(
							Stream.runCollect,
							Effect.forkChild,
						);

						yield* terminals.Handle(
							command("write_real", {
								data: "ping\r",
								terminal_id: "terminal_real",
								type: "terminal.write",
								workspace_id: "workspace_1",
							}),
						);

						const chunks = yield* Fiber.join(output_fiber);
						const bytes = [...chunks].flatMap((event) =>
							event._tag === "chunk" ? [...event.data] : [],
						);

						return new TextDecoder().decode(Uint8Array.from(bytes));
					}),
				),
			);
			const terminal = await wait_for_terminal(
				runtime,
				(current) => current.state === "closed",
			);

			expect(output).toContain("ECHO:ping");
			expect(terminal).toMatchObject({
				exit_code: 9,
				exit_reason: "exited",
				state: "closed",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("claims before dispatch and makes open/write retries at-most-once", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);

			const open = open_command("open_1", "terminal_1");
			const accepted = await route(runtime, open);
			const duplicate = await route(runtime, open);
			const conflict = await route(runtime, {
				...open,
				payload: {
					...(open.payload as Extract<
						CommandEnvelope["payload"],
						{ readonly type: "terminal.open" }
					>),
					cols: 101,
				},
			});
			const write = command("write_1", {
				data: "alpha",
				terminal_id: "terminal_1",
				type: "terminal.write",
				workspace_id: "workspace_1",
			});

			await route(runtime, write);
			await route(runtime, write);
			await create_thread(runtime, "thread_other");

			const denied = await route(
				runtime,
				command(
					"write_cross_thread",
					{
						data: "denied",
						terminal_id: "terminal_1",
						type: "terminal.write",
						workspace_id: "workspace_1",
					},
					"thread_other",
				),
			);
			const denied_workspace = await route(
				runtime,
				command("write_cross_workspace", {
					data: "denied",
					terminal_id: "terminal_1",
					type: "terminal.write",
					workspace_id: "workspace_other",
				}),
			);

			expect(fake.stats.opens).toBe(1);
			expect(fake.stats.writes).toEqual(["alpha"]);
			expect(accepted).toMatchObject([
				{ payload: { status: "accepted" } },
				{ payload: { action: "opened", type: "terminal.lifecycle" } },
			]);
			expect(accepted[0]!.payload).toHaveProperty("journal_sequence", 3);
			expect(duplicate).toMatchObject([
				{ payload: { journal_sequence: 3, status: "duplicate" } },
				{ payload: { action: "opened", type: "terminal.lifecycle" } },
			]);
			expect(lifecycle_event(duplicate)).toEqual(lifecycle_event(accepted));
			expect(conflict).toMatchObject([
				{ payload: { error: { code: "command.id_conflict" }, status: "rejected" } },
			]);
			expect(denied).toMatchObject([
				{ payload: { error: { code: "terminal.not_found" }, status: "rejected" } },
			]);
			expect(denied_workspace).toMatchObject([
				{ payload: { error: { code: "terminal.not_found" }, status: "rejected" } },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("finalizes an event-appended dispatching claim without reapplying or failing it", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_event_window", "terminal_event_window"));

			const write = command("write_event_window", {
				data: "must-not-be-reapplied",
				terminal_id: "terminal_event_window",
				type: "terminal.write",
				workspace_id: "workspace_1",
			});
			const claim = await claim_only(runtime, write);
			const injected_event = await runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;

					return yield* journal.AppendEvent({
						causation_id: write.message_id,
						correlation_id: write.message_id,
						payload: {
							action: "written",
							terminal: claim.stored.terminal,
							type: "terminal.lifecycle",
						},
						thread_id: write.thread_id,
					});
				}),
			);

			const recovered = await route(runtime, write);
			const replayed = await route(runtime, write);

			expect(fake.stats.writes).toEqual([]);
			expect(await list(runtime)).toMatchObject([
				{ generation: 1, state: "active", terminal_id: "terminal_event_window" },
			]);
			expect(recovered).toMatchObject([
				{
					payload: {
						journal_sequence: injected_event.journal_sequence,
						status: "duplicate",
					},
				},
				{ message_id: injected_event.message_id, payload: { action: "written" } },
			]);
			expect(replayed[1]).toEqual(recovered[1]);
		} finally {
			await runtime.dispose();
		}
	});

	it("never applies an old dispatching claim to a restarted generation", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_generation", "terminal_generation"));

			const old_write = command("write_generation_1", {
				data: "stale-generation-write",
				terminal_id: "terminal_generation",
				type: "terminal.write",
				workspace_id: "workspace_1",
			});
			const old_claim = await claim_only(runtime, old_write);

			await route(
				runtime,
				command("kill_generation_1", {
					terminal_id: "terminal_generation",
					type: "terminal.kill",
					workspace_id: "workspace_1",
				}),
			);
			await route(
				runtime,
				command("restart_generation_2", {
					terminal_id: "terminal_generation",
					type: "terminal.restart",
					workspace_id: "workspace_1",
				}),
			);

			const [before_retry] = await list(runtime);
			const closes_before_retry = fake.stats.closes;
			const recovered = await route(runtime, old_write);
			const [after_retry] = await list(runtime);

			expect(old_claim.generation).toBe(1);
			expect(fake.stats.opens).toBe(2);
			expect(fake.stats.closes).toBe(closes_before_retry);
			expect(fake.stats.writes).toEqual([]);
			expect(before_retry).toMatchObject({ generation: 2, state: "active" });
			expect(after_retry).toEqual(before_retry);
			expect(recovered).toMatchObject([
				{ payload: { status: "duplicate" } },
				{
					payload: {
						action: "failed",
						terminal: { generation: 1, state: "failed" },
					},
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("persists resize, clear, kill, restart, close, and ordered lifecycle cursors", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);

			const outputs = [await route(runtime, open_command("open_ops", "terminal_ops"))];

			outputs.push(
				await route(
					runtime,
					command("resize_ops", {
						cols: 140,
						rows: 50,
						terminal_id: "terminal_ops",
						type: "terminal.resize",
						workspace_id: "workspace_1",
					}),
				),
			);
			outputs.push(
				await route(
					runtime,
					command("clear_ops", {
						terminal_id: "terminal_ops",
						type: "terminal.clear",
						workspace_id: "workspace_1",
					}),
				),
			);

			expect(await list(runtime)).toMatchObject([{ cols: 140, rows: 50, state: "active" }]);
			expect(fake.stats.resizes).toEqual([{ cols: 140, rows: 50 }]);
			expect(fake.stats.clears).toBe(1);

			outputs.push(
				await route(
					runtime,
					command("kill_ops", {
						terminal_id: "terminal_ops",
						type: "terminal.kill",
						workspace_id: "workspace_1",
					}),
				),
			);

			expect(await list(runtime)).toMatchObject([{ state: "closed" }]);
			expect(fake.stats.kills).toBe(1);

			outputs.push(
				await route(
					runtime,
					command("restart_ops", {
						terminal_id: "terminal_ops",
						type: "terminal.restart",
						workspace_id: "workspace_1",
					}),
				),
			);

			expect(await list(runtime)).toMatchObject([
				{ cols: 140, generation: 2, rows: 50, state: "active" },
			]);
			expect(fake.stats.opens).toBe(2);

			outputs.push(
				await route(
					runtime,
					command("close_ops", {
						terminal_id: "terminal_ops",
						type: "terminal.close",
						workspace_id: "workspace_1",
					}),
				),
			);

			expect(await list(runtime)).toMatchObject([{ generation: 2, state: "closed" }]);

			const actions = outputs
				.map((output) => lifecycle_event(output)?.payload)
				.map((payload) => (payload && "action" in payload ? payload.action : undefined));
			const sequences = outputs
				.map((output) => output[0]!.payload)
				.map((payload) => ("journal_sequence" in payload ? payload.journal_sequence : 0));

			expect(actions).toEqual([
				"opened",
				"resized",
				"cleared",
				"killed",
				"restarted",
				"closed",
			]);
			expect(sequences.every((sequence) => sequence > 0)).toBe(true);
			expect(sequences).toEqual([...sequences].sort((left, right) => left - right));
		} finally {
			await runtime.dispose();
		}
	});

	it("records natural exit and serves deterministic terminal list queries", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_natural", "terminal_natural"));
			await fake.Exit(0, { exit_code: 7, reason: "exited", signal: null });

			const terminal = await wait_for_terminal(
				runtime,
				(current) => current.state === "closed",
			);

			expect(terminal).toMatchObject({ exit_code: 7, exit_reason: "exited" });

			const query = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;

						yield* connection.Receive(make_hello());
						yield* take(connection, 2);

						const envelope: TerminalListQueryEnvelope = {
							kind: "terminal.list.query",
							message_id: "terminal_list",
							origin: "frontend",
							payload: {
								thread_id: "thread_terminal",
								workspace_id: "workspace_1",
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-07-10T08:00:00.000Z",
						};

						yield* connection.Receive(envelope);

						return yield* take(connection, 1);
					}),
				),
			);

			expect(query).toMatchObject([
				{
					correlation_id: "terminal_list",
					kind: "terminal.list.query.result",
					payload: { terminals: [{ terminal_id: "terminal_natural", state: "closed" }] },
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("binds canonical terminal mutations and owned reads to one thread and workspace", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await create_thread(runtime, "thread_other");
			await route(runtime, open_command("open_owned", "terminal_owned"));

			const ownership_errors = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* TerminalRepository;
					const wrong_thread = yield* repository
						.ReadOwned("terminal_owned", "thread_other", "workspace_1")
						.pipe(Effect.flip);
					const wrong_workspace = yield* repository
						.ReadOwned("terminal_owned", "thread_terminal", "workspace_other")
						.pipe(Effect.flip);

					return { wrong_thread, wrong_workspace };
				}),
			);
			const output_errors = await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;
					const wrong_thread = yield* terminals
						.Output("terminal_owned", "thread_other", "workspace_1")
						.pipe(Effect.flip);
					const wrong_workspace = yield* terminals
						.Output("terminal_owned", "thread_terminal", "workspace_other")
						.pipe(Effect.flip);
					const recent_wrong_thread = yield* terminals
						.RecentOutput("terminal_owned", "thread_other", "workspace_1", 1024)
						.pipe(Effect.flip);
					const recent_wrong_workspace = yield* terminals
						.RecentOutput("terminal_owned", "thread_terminal", "workspace_other", 1024)
						.pipe(Effect.flip);

					return {
						recent_wrong_thread,
						recent_wrong_workspace,
						wrong_thread,
						wrong_workspace,
					};
				}),
			);
			const denied_write = await canonical(
				runtime,
				command("canonical_wrong_workspace", {
					data: "denied",
					terminal_id: "terminal_owned",
					type: "terminal.write",
					workspace_id: "workspace_other",
				}),
				"workspace_other",
			).catch((error) => error);
			const denied_open = await canonical(
				runtime,
				open_command("canonical_open_wrong_workspace", "terminal_wrong_workspace"),
				"workspace_other",
			).catch((error) => error);
			const canonical_open = await canonical(
				runtime,
				open_command("canonical_open", "terminal_canonical"),
				"workspace_1",
			);
			const canonical_write = await canonical(
				runtime,
				command("canonical_write", {
					data: "allowed",
					terminal_id: "terminal_canonical",
					type: "terminal.write",
					workspace_id: "workspace_1",
				}),
				"workspace_1",
			);
			await canonical(
				runtime,
				command("canonical_close", {
					terminal_id: "terminal_canonical",
					type: "terminal.close",
					workspace_id: "workspace_1",
				}),
				"workspace_1",
			);
			const canonical_restart = await canonical(
				runtime,
				command("canonical_restart", {
					terminal_id: "terminal_canonical",
					type: "terminal.restart",
					workspace_id: "workspace_1",
				}),
				"workspace_1",
			);
			await canonical(
				runtime,
				command("canonical_close_restarted", {
					terminal_id: "terminal_canonical",
					type: "terminal.close",
					workspace_id: "workspace_1",
				}),
				"workspace_1",
			);

			expect(ownership_errors.wrong_thread).toBeInstanceOf(TerminalNotFound);
			expect(ownership_errors.wrong_workspace).toBeInstanceOf(TerminalNotFound);
			expect(output_errors.wrong_thread).toBeInstanceOf(TerminalNotFound);
			expect(output_errors.wrong_workspace).toBeInstanceOf(TerminalNotFound);
			expect(output_errors.recent_wrong_thread).toBeInstanceOf(TerminalNotFound);
			expect(output_errors.recent_wrong_workspace).toBeInstanceOf(TerminalNotFound);
			expect(denied_write).toBeInstanceOf(TerminalNotFound);
			expect(denied_open).toBeInstanceOf(TerminalNotFound);
			expect(canonical_open.terminal).toMatchObject({ state: "active" });
			expect(canonical_write.terminal).toMatchObject({ state: "active" });
			expect(canonical_restart.terminal).toMatchObject({ generation: 2, state: "active" });
			expect(fake.stats.writes).toEqual(["allowed"]);
			expect(fake.stats.opens).toBe(3);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects owned output after a durable erasure claim precedes local quiescence", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_claimed_output", "terminal_claimed_output"));
			await fake.Emit(0, "private-before-erasure");
			await wait_for_recent_output(
				runtime,
				"terminal_claimed_output",
				(output) => new TextDecoder().decode(output) === "private-before-erasure",
			);

			const errors = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const terminals = yield* TerminalSessionService;

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-17T15:30:00.000Z",
						thread_id: "thread_terminal",
					});

					const live = yield* terminals
						.Output("terminal_claimed_output", "thread_terminal", "workspace_1")
						.pipe(Effect.flip);
					const recent = yield* terminals
						.RecentOutput(
							"terminal_claimed_output",
							"thread_terminal",
							"workspace_1",
							65_536,
						)
						.pipe(Effect.flip);

					return { live, recent };
				}),
			);

			expect(errors.live).toBeInstanceOf(TerminalNotFound);
			expect(errors.recent).toBeInstanceOf(TerminalNotFound);
		} finally {
			await runtime.dispose();
		}
	});

	it("fans live output into bounded context-owned recent output across terminal generations", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_output", "terminal_output"));
			const empty = await recent_output(runtime, "terminal_output");

			expect(empty).toMatchObject({
				output: new Uint8Array(),
				state: "available",
				truncated: false,
			});

			const live = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const terminals = yield* TerminalSessionService;
						const output = yield* terminals.Output(
							"terminal_output",
							"thread_terminal",
							"workspace_1",
						);
						const received = yield* output.pipe(
							Stream.take(3),
							Stream.runCollect,
							Effect.forkChild,
						);

						yield* Effect.yieldNow;
						yield* Effect.promise(() => fake.Emit(0, "live-"));
						yield* Effect.promise(() => fake.Emit(0, "first-"));
						yield* Effect.promise(() => fake.Emit(0, "chunk-boundary"));

						return yield* Fiber.join(received);
					}),
				),
			);

			const retained = await wait_for_recent_output(runtime, "terminal_output", (output) =>
				new TextDecoder().decode(output).includes("chunk-boundary"),
			);
			const suffix = await recent_output(
				runtime,
				"terminal_output",
				"thread_terminal",
				"workspace_1",
				8,
			);

			expect(
				new TextDecoder().decode(
					Uint8Array.from(
						[...live].flatMap((event) =>
							event._tag === "chunk" ? [...event.data] : [],
						),
					),
				),
			).toBe("live-first-chunk-boundary");
			expect(new TextDecoder().decode(retained.output)).toContain(
				"live-first-chunk-boundary",
			);
			expect(new TextDecoder().decode(suffix.output)).toBe("boundary");
			expect(suffix.truncated).toBe(true);

			retained.output.fill(0);
			expect(
				new TextDecoder().decode((await recent_output(runtime, "terminal_output")).output),
			).toContain("chunk-boundary");

			await fake.Exit(0, { exit_code: 0, reason: "exited", signal: null });
			await wait_for_terminal(runtime, (terminal) => terminal.state === "closed");
			const after_exit = await recent_output(runtime, "terminal_output");

			expect(after_exit.state).toBe("available");
			expect(new TextDecoder().decode(after_exit.output)).toContain("chunk-boundary");
			expect(fake.stats.scope_finalizers).toBe(1);

			await route(
				runtime,
				command("restart_output", {
					terminal_id: "terminal_output",
					type: "terminal.restart",
					workspace_id: "workspace_1",
				}),
			);
			const restarted = await recent_output(runtime, "terminal_output");

			expect(restarted).toMatchObject({
				output: new Uint8Array(),
				state: "available",
				truncated: false,
			});

			await fake.Emit(1, "replacement");
			const replacement = await wait_for_recent_output(
				runtime,
				"terminal_output",
				(output) => new TextDecoder().decode(output) === "replacement",
			);

			expect(new TextDecoder().decode(replacement.output)).toBe("replacement");
			const oversized = new Uint8Array(65_537).fill(65);

			await fake.Emit(1, oversized);
			const oversized_suffix = await wait_for_recent_output(
				runtime,
				"terminal_output",
				(output) => output.length === 65_536,
			);
			const maximum = await recent_output(runtime, "terminal_output");

			expect(oversized_suffix.output).toHaveLength(65_536);
			expect(new TextDecoder().decode(oversized_suffix.output)).toBe("A".repeat(65_536));
			expect(maximum.truncated).toBe(true);
			await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;

					yield* terminals.QuiesceThread("thread_terminal");
				}),
			);
			expect((await recent_output(runtime, "terminal_output")).state).toBe(
				"unavailable_after_restart",
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("eagerly captures output requested before its returned stream starts", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_eager_output", "terminal_eager_output"));
			const output = await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;

					return yield* terminals.Output(
						"terminal_eager_output",
						"thread_terminal",
						"workspace_1",
					);
				}),
			);

			await fake.Emit(0, "captured-before-run");
			const events = await runtime.runPromise(output.pipe(Stream.take(1), Stream.runCollect));

			expect(events).toHaveLength(1);
			expect(events[0]).toMatchObject({ _tag: "chunk", sequence: 1 });
			expect(
				events[0]!._tag === "chunk"
					? new TextDecoder().decode(events[0]!.data)
					: "unexpected-gap",
			).toBe("captured-before-run");
		} finally {
			await runtime.dispose();
		}
	});

	it("isolates slow viewers from driver drain and reports their exact sequence gap", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver({ output_capacity: 2_048 });
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_slow_viewer", "terminal_slow_viewer"));
			const slow_output = await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;

					return yield* terminals.Output(
						"terminal_slow_viewer",
						"thread_terminal",
						"workspace_1",
					);
				}),
			);

			for (let index = 0; index < 600; index += 1) {
				expect(await fake.Emit(0, Uint8Array.of(65))).toBe(true);
			}

			await wait_for_recent_output(
				runtime,
				"terminal_slow_viewer",
				(output) => output.length === 600,
			);
			const fast_events = await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;
					const fast_output = yield* terminals.Output(
						"terminal_slow_viewer",
						"thread_terminal",
						"workspace_1",
					);
					const fast_fiber = yield* fast_output.pipe(
						Stream.take(1),
						Stream.runCollect,
						Effect.forkChild,
					);

					yield* Effect.yieldNow;
					yield* Effect.promise(() => fake.Emit(0, "B"));

					return yield* Fiber.join(fast_fiber);
				}),
			);
			const slow_events = await runtime.runPromise(
				slow_output.pipe(Stream.take(513), Stream.runCollect),
			);
			const terminal = (await list(runtime))[0]!;
			const retained = await recent_output(runtime, "terminal_slow_viewer");

			expect(fast_events).toMatchObject([{ _tag: "chunk", sequence: 601 }]);
			expect(slow_events[0]).toEqual({
				_tag: "gap",
				from_sequence: 1,
				reason: "viewer_overflow",
				to_sequence: 89,
			});
			expect(slow_events[1]).toMatchObject({ _tag: "chunk", sequence: 90 });
			expect(slow_events.at(-1)).toMatchObject({ _tag: "chunk", sequence: 601 });
			expect(terminal.state).toBe("active");
			expect(retained.output).toHaveLength(601);
			expect(retained.output.at(-1)).toBe(66);
		} finally {
			await runtime.dispose();
		}
	});

	it("atomically fences queued and racing output viewers during thread quiescence", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver({
			block_close: true,
			output_on_close: "blocked-during-close",
		});
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_quiesce_output", "terminal_quiesce_output"));

			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const terminals = yield* TerminalSessionService;
						const queued_output = yield* terminals.Output(
							"terminal_quiesce_output",
							"thread_terminal",
							"workspace_1",
						);
						const live_output = yield* terminals.Output(
							"terminal_quiesce_output",
							"thread_terminal",
							"workspace_1",
						);
						const observed = yield* Ref.make<ReadonlyArray<string>>([]);
						const observed_before_fence = yield* Deferred.make<void>();
						const live_fiber = yield* live_output.pipe(
							Stream.runForEach((event) => {
								if (event._tag === "gap") {
									return Effect.void;
								}

								const text = new TextDecoder().decode(event.data);

								return Ref.update(observed, (current) => [...current, text]).pipe(
									Effect.andThen(
										text === "allowed-before"
											? Deferred.succeed(observed_before_fence, undefined)
											: Effect.void,
									),
								);
							}),
							Effect.exit,
							Effect.forkChild,
						);

						yield* Effect.promise(() => fake.Emit(0, "allowed-before"));
						yield* Deferred.await(observed_before_fence);

						const quiesce_fiber = yield* terminals
							.QuiesceThread("thread_terminal")
							.pipe(Effect.forkChild);

						yield* Effect.promise(() => fake.WaitForClose(0));

						const racing_output = yield* terminals
							.Output("terminal_quiesce_output", "thread_terminal", "workspace_1")
							.pipe(Effect.exit, Effect.forkChild);

						yield* Effect.yieldNow;

						const racing_before_release = racing_output.pollUnsafe();

						yield* Effect.promise(() => fake.ReleaseClose(0));
						yield* Fiber.join(quiesce_fiber);

						const racing_exit = yield* Fiber.join(racing_output);
						const live_exit = yield* Fiber.join(live_fiber);
						const queued_observed = yield* Ref.make<ReadonlyArray<string>>([]);
						const queued_exit = yield* queued_output.pipe(
							Stream.runForEach((event) =>
								event._tag === "gap"
									? Effect.void
									: Ref.update(queued_observed, (current) => [
											...current,
											new TextDecoder().decode(event.data),
										]),
							),
							Effect.exit,
						);

						return {
							live_exit,
							observed: yield* Ref.get(observed),
							queued_exit,
							queued_observed: yield* Ref.get(queued_observed),
							racing_before_release,
							racing_exit,
							recent: yield* terminals.RecentOutput(
								"terminal_quiesce_output",
								"thread_terminal",
								"workspace_1",
								65_536,
							),
						};
					}),
				),
			);

			expect(result.racing_before_release).toBeUndefined();
			expect(result.observed).toEqual(["allowed-before"]);
			expect(result.queued_observed).toEqual([]);
			expect(result.recent).toMatchObject({
				output: new Uint8Array(),
				state: "unavailable_after_restart",
			});
			expect(result.live_exit._tag).toBe("Failure");
			expect(result.queued_exit._tag).toBe("Failure");
			expect(result.racing_exit._tag).toBe("Failure");

			if (result.racing_exit._tag === "Failure") {
				expect(Cause.squash(result.racing_exit.cause)).toBeInstanceOf(TerminalNotFound);
			}
		} finally {
			if (fake.stats.opens > 0) {
				await fake.ReleaseClose(0);
			}

			await runtime.dispose();
		}
	});

	it("erases an unstarted viewer backlog after normal terminal exit", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_ended_backlog", "terminal_ended_backlog"));

			const output = await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;

					return yield* terminals.Output(
						"terminal_ended_backlog",
						"thread_terminal",
						"workspace_1",
					);
				}),
			);

			await fake.Emit(0, "queued-before-exit");
			await wait_for_recent_output(
				runtime,
				"terminal_ended_backlog",
				(bytes) => new TextDecoder().decode(bytes) === "queued-before-exit",
			);
			await fake.Exit(0, { exit_code: 0, reason: "exited", signal: null });
			await wait_for_terminal(runtime, (terminal) => terminal.state === "closed");
			await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;

					yield* terminals.QuiesceThread("thread_terminal");
				}),
			);

			const observed: Array<string> = [];
			const output_exit = await runtime.runPromise(
				output.pipe(
					Stream.runForEach((event) =>
						Effect.sync(() => {
							if (event._tag === "chunk") {
								observed.push(new TextDecoder().decode(event.data));
							}
						}),
					),
					Effect.exit,
				),
			);
			const recent = await recent_output(runtime, "terminal_ended_backlog");

			expect(output_exit._tag).toBe("Success");
			expect(observed).toEqual([]);
			expect(recent).toMatchObject({
				output: new Uint8Array(),
				state: "unavailable_after_restart",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("drains every queued driver chunk before committing a normal exit", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver({ output_capacity: 1_024 });
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});
		const expected = Uint8Array.from(Array.from({ length: 700 }, (_, index) => index % 251));

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_queued_exit", "terminal_queued_exit"));

			for (const byte of expected) {
				expect(await fake.Emit(0, Uint8Array.of(byte))).toBe(true);
			}

			await fake.Exit(0, { exit_code: 0, reason: "exited", signal: null });
			const terminal = await wait_for_terminal(
				runtime,
				(current) => current.state === "closed",
			);
			const retained = await recent_output(runtime, "terminal_queued_exit");

			expect(terminal).toMatchObject({ exit_code: 0, exit_reason: "exited" });
			expect(retained.output).toEqual(expected);
			expect(retained.truncated).toBe(false);
		} finally {
			await runtime.dispose();
		}
	});

	it("fails a terminal when its driver exits without completing output", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver({ exit_before_output_end: true });
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(
				runtime,
				open_command("open_nonconforming_output", "terminal_nonconforming_output"),
			);
			await fake.ExitWithoutOutputEnd(0, {
				exit_code: 0,
				reason: "exited",
				signal: null,
			});

			const terminal = await wait_for_terminal(
				runtime,
				(current) => current.state === "failed",
			);

			expect(terminal).toMatchObject({
				failure: expect.stringContaining("output stream completed"),
				state: "failed",
			});
			expect(fake.stats.scope_finalizers).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("returns unavailable recent output after the backend runtime restarts and validates read bounds", async () => {
		const database_path = await make_database_path();
		const first_fake = make_fake_terminal_driver();
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: first_fake.layer,
		});

		try {
			await create_thread(first_runtime);
			await route(first_runtime, open_command("open_restart", "terminal_restart"));
			await first_fake.Emit(0, "ephemeral");
			await wait_for_recent_output(
				first_runtime,
				"terminal_restart",
				(output) => new TextDecoder().decode(output) === "ephemeral",
			);
			const invalid = await recent_output(
				first_runtime,
				"terminal_restart",
				"thread_terminal",
				"workspace_1",
				0,
			).catch((error) => error);
			const too_large = await recent_output(
				first_runtime,
				"terminal_restart",
				"thread_terminal",
				"workspace_1",
				65_537,
			).catch((error) => error);

			expect(invalid).toHaveProperty("_tag", "TerminalInvariantError");
			expect(too_large).toHaveProperty("_tag", "TerminalInvariantError");
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: make_fake_terminal_driver().layer,
		});

		try {
			const unavailable = await recent_output(second_runtime, "terminal_restart");

			expect(unavailable).toMatchObject({
				output: new Uint8Array(),
				state: "unavailable_after_restart",
				truncated: false,
			});
		} finally {
			await second_runtime.dispose();
		}
	});

	it("terminalizes spawn and post-spawn persistence failures without leaking scopes", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);

			const spawn_failure = await route(
				runtime,
				open_command("open_spawn_failure", "terminal_spawn_failure", "spawn-failure"),
			);

			expect(spawn_failure).toMatchObject([
				{ payload: { status: "accepted" } },
				{
					payload: {
						action: "failed",
						terminal: { state: "failed", terminal_id: "terminal_spawn_failure" },
					},
				},
			]);
			expect(fake.stats.scope_finalizers).toBe(1);

			const persistence_command = open_command(
				"open_persistence_failure",
				"terminal_persistence_failure",
				"invalid-pid",
			);
			const persistence_failure = await route(runtime, persistence_command);

			expect(persistence_failure).toMatchObject([
				{
					payload: {
						error: { code: "terminal.unavailable" },
						status: "rejected",
					},
				},
			]);
			expect(fake.stats.scope_finalizers).toBe(2);

			const recovered_retry = await route(runtime, persistence_command);

			expect(fake.stats.opens).toBe(2);
			expect(recovered_retry).toMatchObject([
				{ payload: { status: "duplicate" } },
				{
					payload: {
						action: "failed",
						terminal: { state: "failed", terminal_id: "terminal_persistence_failure" },
					},
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("recovers a stale active session only after its owning backend crashes", async () => {
		const database_path = await make_database_path();

		await leave_crashed_terminal(database_path);

		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			const terminals = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* TerminalSessionService;

					return yield* service.List("thread_crash", "workspace_crash");
				}),
			);

			expect(fake.stats.opens).toBe(0);
			expect(terminals).toMatchObject([
				{
					failure: expect.stringContaining("backend stopped"),
					state: "failed",
					terminal_id: "terminal_crash",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	}, 30_000);
});

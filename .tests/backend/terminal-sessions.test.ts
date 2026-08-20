import { mkdtemp, rm } from "node:fs/promises";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { Cause, Deferred, Effect, Fiber, Layer, ManagedRuntime, Queue, Stream } from "effect";

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
	TerminalRepository,
	TerminalSessionService,
	TerminalSessionServiceLive,
	type ProtocolConnection,
	type TerminalDriverExit,
} from "@artisan/backend";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { Database } from "../../modules/backend/src/persistence/database";
import { OrchestrationRuns } from "../../modules/backend/src/persistence/tables";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const crash_fixture_path = fileURLToPath(new URL("./terminal-crash-fixture.ts", import.meta.url));
const typescript_loader_url = new URL("./terminal-typescript-loader.ts", import.meta.url).href;
const temporary_directories: Array<string> = [];

interface FakeTerminal {
	readonly exit: Deferred.Deferred<TerminalDriverExit>;
	readonly output: Queue.Queue<Uint8Array, Cause.Done<void>>;
	closed: boolean;
}

function make_fake_terminal_driver() {
	const terminals: Array<FakeTerminal> = [];
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
				const output = yield* Queue.unbounded<Uint8Array, Cause.Done<void>>();
				const terminal: FakeTerminal = { closed: false, exit, output };
				const withhold_exit = input.executable === "withhold-exit";

				terminals.push(terminal);

				const Finish = (terminal_exit: TerminalDriverExit) =>
					Effect.gen(function* () {
						if (terminal.closed) {
							return;
						}

						terminal.closed = true;
						yield* Queue.end(output);
						yield* Deferred.succeed(exit, terminal_exit);
					});
				const Close = Effect.gen(function* () {
					if (terminal.closed) {
						return;
					}

					stats.closes += 1;
					if (withhold_exit) {
						return;
					}
					yield* Finish({ exit_code: null, reason: "closed", signal: null });
				});

				return {
					Clear: Effect.sync(() => {
						stats.clears += 1;
					}),
					Close,
					Exit: Deferred.await(exit),
					Kill: () =>
						Effect.gen(function* () {
							stats.kills += 1;
							if (withhold_exit) {
								return;
							}
							yield* Finish({ exit_code: null, reason: "killed", signal: null });
						}),
					Output: Stream.fromQueue(output),
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
		Emit: (index: number, text: string) =>
			Effect.runPromise(
				Queue.offer(terminals[index]!.output, new TextEncoder().encode(text)).pipe(
					Effect.asVoid,
				),
			),
		Exit: (index: number, exit: TerminalDriverExit) =>
			Effect.runPromise(Deferred.succeed(terminals[index]!.exit, exit)),
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
	for (let attempt = 0; attempt < 100; attempt += 1) {
		const [terminal] = await list(runtime);

		if (terminal && predicate(terminal)) {
			return terminal;
		}

		await new Promise<void>((resolve) => setTimeout(resolve, 10));
	}

	throw new Error("Terminal did not reach the expected state");
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
	it("keeps caller-supplied agent provenance user-owned on the public command path", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		let runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;
					yield* Effect.forEach(
						[
							["forged", "agent_forged", "run_forged"],
							["nonexistent", "agent_missing", "run_missing"],
							["cross_thread", "agent_other", "run_other"],
						] as const,
						([suffix, agent_id, run_id]) =>
							terminals.Handle({
								...open_command(`forged_agent_${suffix}`, `terminal_${suffix}`),
								agent_id,
								run_id,
							}),
						{ discard: true },
					);
				}),
			);
			const terminals = await list(runtime);
			expect(terminals.map((terminal) => terminal.ownership)).toEqual([
				{ kind: "user" },
				{ kind: "user" },
				{ kind: "user" },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("adopts agent ownership only through the internal durable-authority path", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		let runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});
		try {
			await create_thread(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.insert(OrchestrationRuns).values({
						agent_id: "agent_valid",
						created_at: "2026-07-10T08:00:00.000Z",
						engine_id: "codex",
						run_id: "run_valid",
						status: "running",
						thread_id: "thread_terminal",
						updated_at: "2026-07-10T08:00:00.000Z",
						working_directory: tmpdir(),
					});
					const terminals = yield* TerminalSessionService;
					yield* terminals.HandleAsAgent(
						open_command("valid_agent_open", "terminal_valid"),
						{
							agent_id: "agent_valid",
							run_id: "run_valid",
						},
					);
				}),
			);
			const [terminal] = await list(runtime);
			expect(terminal?.ownership).toEqual({
				agent_id: "agent_valid",
				kind: "agent",
				run_id: "run_valid",
			});
			await runtime.dispose();
			runtime = make_backend_runtime({
				database_path,
				migrations_path,
				terminal_driver: fake.layer,
			});
			const [restarted] = await list(runtime);
			expect(restarted?.ownership).toEqual({
				agent_id: "agent_valid",
				kind: "agent",
				run_id: "run_valid",
			});
		} finally {
			await runtime.dispose();
		}
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
						const stream = yield* terminals.Output({
							terminal_id: "terminal_real",
							thread_id: "thread_terminal",
							workspace_id: "workspace_1",
						});
						const output_fiber = yield* stream.pipe(
							Stream.runCollect,
							Effect.forkChild,
						);
						const cross_scope = yield* terminals
							.Output({
								terminal_id: "terminal_real",
								thread_id: "other_thread",
								workspace_id: "workspace_1",
							})
							.pipe(Effect.exit);
						if (cross_scope._tag !== "Failure") {
							return yield* Effect.die(
								new Error("cross-thread terminal output was exposed"),
							);
						}
						const cross_workspace = yield* terminals
							.Output({
								terminal_id: "terminal_real",
								thread_id: "thread_terminal",
								workspace_id: "other_workspace",
							})
							.pipe(Effect.exit);
						if (cross_workspace._tag !== "Failure") {
							return yield* Effect.die(
								new Error("cross-workspace terminal output was exposed"),
							);
						}

						yield* terminals.Handle(
							command("write_real", {
								data: "ping\r",
								terminal_id: "terminal_real",
								type: "terminal.write",
							}),
						);

						const chunks = yield* Fiber.join(output_fiber);

						return new TextDecoder().decode(
							Uint8Array.from([...chunks].flatMap((chunk) => [...chunk])),
						);
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
			});

			await route(runtime, write);
			await route(runtime, write);
			await create_thread(runtime, "thread_other");

			const denied = await route(
				runtime,
				command(
					"write_cross_thread",
					{ data: "denied", terminal_id: "terminal_1", type: "terminal.write" },
					"thread_other",
				),
			);

			expect(fake.stats.opens).toBe(1);
			expect(fake.stats.writes).toEqual(["alpha"]);
			expect(accepted).toMatchObject([
				{ payload: { status: "accepted" } },
				{ payload: { action: "opened", type: "terminal.lifecycle" } },
			]);
			const accepted_journal_sequence =
				"journal_sequence" in accepted[0]!.payload
					? accepted[0]!.payload.journal_sequence
					: undefined;
			expect(accepted_journal_sequence).toBeTypeOf("number");
			expect(duplicate).toMatchObject([
				{
					payload: {
						journal_sequence: accepted_journal_sequence,
						status: "duplicate",
					},
				},
				{ payload: { action: "opened", type: "terminal.lifecycle" } },
			]);
			expect(lifecycle_event(duplicate)).toEqual(lifecycle_event(accepted));
			expect(conflict).toMatchObject([
				{ payload: { error: { code: "command.id_conflict" }, status: "rejected" } },
			]);
			expect(denied).toMatchObject([
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
			});
			const old_claim = await claim_only(runtime, old_write);

			await route(
				runtime,
				command("kill_generation_1", {
					terminal_id: "terminal_generation",
					type: "terminal.kill",
				}),
			);
			await wait_for_terminal(runtime, (terminal) => terminal.state === "closed");
			await route(
				runtime,
				command("restart_generation_2", {
					terminal_id: "terminal_generation",
					type: "terminal.restart",
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
					}),
				),
			);
			outputs.push(
				await route(
					runtime,
					command("clear_ops", { terminal_id: "terminal_ops", type: "terminal.clear" }),
				),
			);

			expect(await list(runtime)).toMatchObject([{ cols: 140, rows: 50, state: "active" }]);
			expect(fake.stats.resizes).toEqual([{ cols: 140, rows: 50 }]);
			expect(fake.stats.clears).toBe(1);

			outputs.push(
				await route(
					runtime,
					command("kill_ops", { terminal_id: "terminal_ops", type: "terminal.kill" }),
				),
			);
			await wait_for_terminal(runtime, (terminal) => terminal.state === "closed");

			expect(await list(runtime)).toMatchObject([{ state: "closed" }]);
			expect(fake.stats.kills).toBe(1);

			outputs.push(
				await route(
					runtime,
					command("restart_ops", {
						terminal_id: "terminal_ops",
						type: "terminal.restart",
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
					command("close_ops", { terminal_id: "terminal_ops", type: "terminal.close" }),
				),
			);
			await wait_for_terminal(runtime, (terminal) => terminal.state === "closed");

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

	it("bounds a missing kill exit, records ambiguity, and releases the command lane", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(
				runtime,
				open_command("open_withheld_exit", "terminal_withheld_exit", "withhold-exit"),
			);

			const kill = route(
				runtime,
				command("kill_withheld_exit", {
					terminal_id: "terminal_withheld_exit",
					type: "terminal.kill",
				}),
			);
			for (let attempt = 0; fake.stats.kills === 0 && attempt < 100; attempt += 1) {
				await new Promise<void>((resolve) => setTimeout(resolve, 10));
			}
			expect(fake.stats.kills).toBe(1);

			const killed = await kill;
			const second = await Promise.race([
				route(
					runtime,
					open_command("open_after_withheld_exit", "terminal_after_withheld_exit"),
				),
				new Promise<never>((_, reject) =>
					setTimeout(
						() => reject(new Error("terminal command lane remained blocked")),
						400,
					),
				),
			]);

			expect(killed.at(-1)).toMatchObject({
				payload: {
					action: "killed",
					terminal: { state: "active", terminal_id: "terminal_withheld_exit" },
				},
			});
			expect(
				await route(
					runtime,
					command("write_after_kill_receipt", {
						data: "must-not-reach-a-stopping-terminal",
						terminal_id: "terminal_withheld_exit",
						type: "terminal.write",
					}),
				),
			).toMatchObject([
				{ payload: { error: { code: "terminal.not_active" }, status: "rejected" } },
			]);
			expect(second.at(-1)).toMatchObject({
				payload: { action: "opened", terminal: { state: "active" } },
			});
			for (let attempt = 0; attempt < 100; attempt += 1) {
				const recovered = (await list(runtime)).find(
					(terminal) => terminal.terminal_id === "terminal_withheld_exit",
				);

				if (recovered?.state === "failed") {
					break;
				}
				await new Promise<void>((resolve) => setTimeout(resolve, 10));
			}
			expect(
				(await list(runtime)).find(
					(terminal) => terminal.terminal_id === "terminal_withheld_exit",
				),
			).toMatchObject({ state: "failed" });
			expect(fake.stats.scope_finalizers).toBe(1);

			await route(
				runtime,
				command("restart_after_withheld_exit", {
					terminal_id: "terminal_withheld_exit",
					type: "terminal.restart",
				}),
			);
			await fake.Exit(0, { exit_code: 1, reason: "exited", signal: null });
			expect(
				(await list(runtime)).find(
					(terminal) => terminal.terminal_id === "terminal_withheld_exit",
				),
			).toMatchObject({ generation: 2, state: "active" });
			await fake.Exit(2, { exit_code: 0, reason: "exited", signal: null });
		} finally {
			await runtime.dispose();
		}
	});

	it("settles lost exits concurrently during shutdown", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		await create_thread(runtime);
		await Promise.all(
			["alpha", "bravo", "charlie"].map((terminal_id) =>
				route(
					runtime,
					open_command(`open_shutdown_${terminal_id}`, terminal_id, "withhold-exit"),
				),
			),
		);

		const started_at = performance.now();
		await runtime.dispose();
		const elapsed = performance.now() - started_at;

		expect(elapsed).toBeLessThan(2_500);
		expect(fake.stats.scope_finalizers).toBe(3);
	});

	it("retires a displaced stopped generation when restart wins before its watchdog", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(
				runtime,
				open_command("open_displaced", "terminal_displaced", "withhold-exit"),
			);
			await route(
				runtime,
				command("kill_displaced", {
					terminal_id: "terminal_displaced",
					type: "terminal.kill",
				}),
			);
			await route(
				runtime,
				command("restart_displaced", {
					terminal_id: "terminal_displaced",
					type: "terminal.restart",
				}),
			);

			for (
				let attempt = 0;
				fake.stats.scope_finalizers === 0 && attempt < 100;
				attempt += 1
			) {
				await new Promise<void>((resolve) => setTimeout(resolve, 10));
			}
			expect(fake.stats.scope_finalizers).toBe(1);
			await Effect.runPromise(Effect.sleep("1100 millis"));
			await fake.Exit(0, { exit_code: 1, reason: "exited", signal: null });
			expect(await list(runtime)).toMatchObject([
				{ generation: 2, state: "active", terminal_id: "terminal_displaced" },
			]);
			await fake.Exit(1, { exit_code: 0, reason: "exited", signal: null });
		} finally {
			await runtime.dispose();
		}
	});

	it("constructs before stale recovery settles and admits a waiting terminal command after recovery", async () => {
		const recovery_started = await Effect.runPromise(Deferred.make<void>());
		const release_recovery = await Effect.runPromise(Deferred.make<void>());
		const fake = make_fake_terminal_driver();
		let admitted = false;
		const stored = { terminal: {} } as never;
		const repository = TerminalRepository.of({
			AdoptObserved: () => Effect.void,
			Claim: () =>
				Effect.sync(() => {
					admitted = true;
					return {
						command_status: "dispatching",
						generation: 1,
						status: "accepted",
						stored,
					} as never;
				}),
			CommitAmbiguous: () => Effect.die("unreachable"),
			CommitCommand: () =>
				Effect.succeed({ event: { journal_sequence: 1 }, stored } as never),
			CommitExit: () => Effect.die("unreachable"),
			CommitRecovery: () => Effect.die("unreachable"),
			CompleteCommand: () => Effect.die("unreachable"),
			List: () => Effect.succeed([]),
			ReadOwned: () => Effect.die("unreachable"),
			ReadStale: () => Effect.succeed([]),
			RecoverStale: () =>
				Deferred.succeed(recovery_started, undefined).pipe(
					Effect.andThen(Deferred.await(release_recovery)),
					Effect.as(0),
				),
		});
		const runtime = ManagedRuntime.make(
			TerminalSessionServiceLive.pipe(
				Layer.provideMerge(fake.layer),
				Layer.provideMerge(Layer.succeed(TerminalRepository, repository)),
				Layer.provideMerge(Layer.succeed(JournalStore, {} as never)),
				Layer.provideMerge(
					Layer.succeed(
						RuntimeMetadata,
						RuntimeMetadata.of({
							instance_id: "stalled-recovery-instance",
							MakeId: () => Effect.succeed("unused"),
							Now: Effect.succeed("2026-08-15T00:00:00.000Z"),
						}),
					),
				),
			),
		);

		try {
			const service = await runtime.runPromise(
				Effect.gen(function* () {
					return yield* TerminalSessionService;
				}),
			);
			await Effect.runPromise(Deferred.await(recovery_started));
			expect(
				await runtime.runPromise(service.List("thread_terminal", "workspace_1")),
			).toEqual([]);

			const pending = runtime.runPromise(
				service.Handle(
					command("gated_during_recovery", {
						pinned: true,
						terminal_id: "terminal_gated",
						type: "terminal.pin",
					}),
				),
			);
			await new Promise<void>((resolve) => setTimeout(resolve, 30));
			expect(admitted).toBe(false);

			await Effect.runPromise(Deferred.succeed(release_recovery, undefined));
			await expect(pending).resolves.toMatchObject({ status: "accepted" });
			expect(admitted).toBe(true);
		} finally {
			await runtime.dispose().catch(() => undefined);
		}
	});

	it("interrupts recovery-gated terminal waiters when the service closes", async () => {
		const recovery_started = await Effect.runPromise(Deferred.make<void>());
		const release_recovery = await Effect.runPromise(Deferred.make<void>());
		const fake = make_fake_terminal_driver();
		const repository = TerminalRepository.of({
			AdoptObserved: () => Effect.void,
			Claim: () => Effect.die("terminal command passed a closed recovery gate"),
			CommitAmbiguous: () => Effect.die("unreachable"),
			CommitCommand: () => Effect.die("unreachable"),
			CommitExit: () => Effect.die("unreachable"),
			CommitRecovery: () => Effect.die("unreachable"),
			CompleteCommand: () => Effect.die("unreachable"),
			List: () => Effect.succeed([]),
			ReadOwned: () => Effect.die("unreachable"),
			ReadStale: () => Effect.succeed([]),
			RecoverStale: () =>
				Deferred.succeed(recovery_started, undefined).pipe(
					Effect.andThen(Deferred.await(release_recovery)),
					Effect.as(0),
				),
		});
		const runtime = ManagedRuntime.make(
			TerminalSessionServiceLive.pipe(
				Layer.provideMerge(fake.layer),
				Layer.provideMerge(Layer.succeed(TerminalRepository, repository)),
				Layer.provideMerge(Layer.succeed(JournalStore, {} as never)),
				Layer.provideMerge(
					Layer.succeed(
						RuntimeMetadata,
						RuntimeMetadata.of({
							instance_id: "stalled-recovery-close-instance",
							MakeId: () => Effect.succeed("unused"),
							Now: Effect.succeed("2026-08-15T00:00:00.000Z"),
						}),
					),
				),
			),
		);

		try {
			const service = await runtime.runPromise(
				Effect.gen(function* () {
					return yield* TerminalSessionService;
				}),
			);
			await Effect.runPromise(Deferred.await(recovery_started));
			const pending = runtime.runPromise(
				service.Handle(open_command("gated_during_close", "terminal_gated_close")),
			);

			await runtime.dispose();
			await expect(pending).rejects.toBeDefined();
		} finally {
			await runtime.dispose().catch(() => undefined);
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

	it("replays scrollback to late, repeated, and post-exit readers", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});
		const scope = {
			terminal_id: "terminal_scrollback",
			thread_id: "thread_terminal",
			workspace_id: "workspace_1",
		};
		const read_all = () =>
			runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;
					const stream = yield* terminals.Output(scope);
					const chunks = yield* stream.pipe(Stream.runCollect);

					return new TextDecoder().decode(
						Uint8Array.from([...chunks].flatMap((chunk) => [...chunk])),
					);
				}),
			);

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_scrollback", "terminal_scrollback"));
			await fake.Emit(0, "alpha");

			const late_reader = await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;
					const stream = yield* terminals.Output(scope);
					const chunks = yield* stream.pipe(Stream.take(1), Stream.runCollect);

					return new TextDecoder().decode(
						Uint8Array.from([...chunks].flatMap((chunk) => [...chunk])),
					);
				}),
			);

			expect(late_reader).toContain("alpha");

			await fake.Emit(0, "beta");
			await route(
				runtime,
				command("kill_scrollback", {
					terminal_id: "terminal_scrollback",
					type: "terminal.kill",
				}),
			);

			/** The postmortem read is the headline: full output, twice, after exit. */
			expect(await read_all()).toBe("alphabeta");
			expect(await read_all()).toBe("alphabeta");

			const snapshot = await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;
					const stream = yield* terminals.ReadOutput("terminal_scrollback");
					const chunks = yield* stream.pipe(Stream.runCollect);

					return new TextDecoder().decode(
						Uint8Array.from([...chunks].flatMap((chunk) => [...chunk])),
					);
				}),
			);

			expect(snapshot).toBe("alphabeta");

			const quiesced = await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;

					yield* terminals.QuiesceThread("thread_terminal");

					return yield* terminals.Output(scope).pipe(Effect.exit);
				}),
			);

			expect(quiesced._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});

	it("bounds scrollback by dropping the oldest output without stopping the terminal", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});
		const big = "a".repeat(512 * 1024);

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_bounded", "terminal_bounded"));
			await fake.Emit(0, big);
			await fake.Emit(0, big);
			await fake.Emit(0, "OMEGA");

			expect(await list(runtime)).toMatchObject([{ state: "active" }]);

			await route(
				runtime,
				command("kill_bounded", { terminal_id: "terminal_bounded", type: "terminal.kill" }),
			);
			await wait_for_terminal(runtime, (terminal) => terminal.state === "closed");

			const text = await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;
					const stream = yield* terminals.Output({
						terminal_id: "terminal_bounded",
						thread_id: "thread_terminal",
						workspace_id: "workspace_1",
					});
					const chunks = yield* stream.pipe(Stream.runCollect);

					return new TextDecoder().decode(
						Uint8Array.from([...chunks].flatMap((chunk) => [...chunk])),
					);
				}),
			);

			expect(text.length).toBe(big.length + "OMEGA".length);
			expect(text.endsWith("OMEGA")).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("resets scrollback for a restarted generation", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_terminal_driver();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			terminal_driver: fake.layer,
		});

		try {
			await create_thread(runtime);
			await route(runtime, open_command("open_reset", "terminal_reset"));
			await fake.Emit(0, "old-generation");
			await route(
				runtime,
				command("kill_reset", { terminal_id: "terminal_reset", type: "terminal.kill" }),
			);
			await wait_for_terminal(runtime, (terminal) => terminal.state === "closed");
			await route(
				runtime,
				command("restart_reset", {
					terminal_id: "terminal_reset",
					type: "terminal.restart",
				}),
			);
			await fake.Emit(1, "new-generation");

			const text = await runtime.runPromise(
				Effect.gen(function* () {
					const terminals = yield* TerminalSessionService;
					const stream = yield* terminals.Output({
						terminal_id: "terminal_reset",
						thread_id: "thread_terminal",
						workspace_id: "workspace_1",
					});
					const chunks = yield* stream.pipe(Stream.take(1), Stream.runCollect);

					return new TextDecoder().decode(
						Uint8Array.from([...chunks].flatMap((chunk) => [...chunk])),
					);
				}),
			);

			expect(text).toBe("new-generation");
		} finally {
			await runtime.dispose();
		}
	});
});

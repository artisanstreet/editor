import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Cause, Deferred, Effect, Fiber, Layer, Queue, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	Engine,
	EngineCommand,
	EngineObservation,
	EngineOpenInput,
	EngineRun,
	EngineRunTerminalState,
} from "@artisan/engines";
import type { AuthoritativeCommandEnvelope } from "../../modules/backend/src/persistence/orchestration/message-command";
import {
	AgentGraphOrchestrator,
	AgentGraphInvalid,
	AgentGraphRepository,
	AgentOrchestrator,
	make_backend_runtime,
	ProtocolRouter,
	TerminalDriver,
	TerminalNotFound,
	TerminalSessionService,
	ThreadErasure,
} from "@artisan/backend";
import { Database } from "../../modules/backend/src/persistence/database";
import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration-repository";
import {
	ThreadErasureClaims,
	OrchestrationGraphCommands,
	TerminalCommands,
	TerminalSessions,
	PreviewCommands,
	PreviewDispatchLeases,
	PreviewInspectionSessions,
	PreviewTargets,
	JournalEvents,
	ThreadTombstones,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

function make_capabilities() {
	return Object.fromEntries(
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
}

function make_counting_engine(id: string) {
	let opens = 0;

	const engine = {
		Descriptor: {
			capabilities: make_capabilities(),
			display_name: `Counting engine ${id}`,
			id,
			transport: "test",
		},
		Open: (input) =>
			Effect.sync(() => {
				opens += 1;

				return {
					artisan_run_id: input.artisan_run_id,
					Closed: Effect.never,
					Events: Stream.never,
					native_thread_id: `native:${input.artisan_run_id}`,
					resume_token: { native_thread_id: `native:${input.artisan_run_id}` },
					Send: () => Effect.void,
				} satisfies EngineRun;
			}),
		Probe: () => Effect.die("Probe is not used by dispatch fence tests"),
	} satisfies Engine;

	return { engine, opens: () => opens };
}

async function make_blocking_engine(id: string) {
	const open_release = await Effect.runPromise(Deferred.make<void>());
	const open_started = await Effect.runPromise(Queue.unbounded<EngineOpenInput>());
	const observer_started = await Effect.runPromise(Queue.unbounded<string>());
	const send_release = await Effect.runPromise(Deferred.make<void>());
	const send_started = await Effect.runPromise(Queue.unbounded<EngineCommand>());
	const scope_closed = await Effect.runPromise(Queue.unbounded<string>());
	let active_opens = 0;
	let active_runs = 0;
	let active_sends = 0;
	let max_active_opens = 0;

	const engine = {
		Descriptor: {
			capabilities: make_capabilities(),
			display_name: `Blocking engine ${id}`,
			id,
			transport: "test",
		},
		Open: (input: EngineOpenInput) =>
			Effect.gen(function* () {
				const closed = yield* Deferred.make<EngineRunTerminalState>();
				const events = yield* Queue.unbounded<EngineObservation, Cause.Done<void>>();
				let run_active = false;

				active_opens += 1;
				max_active_opens = Math.max(max_active_opens, active_opens);
				yield* Effect.addFinalizer(() =>
					Effect.gen(function* () {
						if (run_active) {
							active_runs -= 1;
						}

						yield* Queue.end(events);
						yield* Deferred.succeed(closed, "closed");
						yield* Queue.offer(scope_closed, input.artisan_run_id);
					}),
				);
				yield* Queue.offer(open_started, input);
				yield* Deferred.await(open_release).pipe(
					Effect.ensuring(
						Effect.sync(() => {
							active_opens -= 1;
						}),
					),
				);

				run_active = true;
				active_runs += 1;

				return {
					artisan_run_id: input.artisan_run_id,
					Closed: Deferred.await(closed),
					Events: Stream.unwrap(
						Queue.offer(observer_started, input.artisan_run_id).pipe(
							Effect.as(Stream.fromQueue(events)),
						),
					),
					native_thread_id: `native:${input.artisan_run_id}`,
					resume_token: { native_thread_id: `native:${input.artisan_run_id}` },
					Send: (engine_command) =>
						Effect.gen(function* () {
							active_sends += 1;
							yield* Queue.offer(send_started, engine_command);
							yield* Deferred.await(send_release).pipe(
								Effect.ensuring(
									Effect.sync(() => {
										active_sends -= 1;
									}),
								),
							);
						}),
				} satisfies EngineRun;
			}),
		Probe: () => Effect.die("Probe is not used by dispatch fence tests"),
	} satisfies Engine;

	return {
		active_opens: () => active_opens,
		active_runs: () => active_runs,
		active_sends: () => active_sends,
		engine,
		max_active_opens: () => max_active_opens,
		observer_started,
		open_release,
		open_started,
		send_release,
		send_started,
		scope_closed,
	};
}

function make_terminal_driver() {
	let opens = 0;
	const layer = Layer.succeed(TerminalDriver, {
		Open: () =>
			Effect.gen(function* () {
				const exit = yield* Deferred.make<{
					readonly exit_code: number | null;
					readonly reason: "closed";
					readonly signal: number | null;
				}>();

				opens += 1;

				const Close = Deferred.succeed(exit, {
					exit_code: null,
					reason: "closed" as const,
					signal: null,
				}).pipe(Effect.asVoid);

				yield* Effect.addFinalizer(() => Close);

				return {
					Clear: Effect.void,
					Close,
					Exit: Deferred.await(exit),
					Kill: () => Close,
					Output: Stream.empty,
					Resize: () => Effect.void,
					Write: () => Effect.void,
					pid: 20_001,
				};
			}),
	});

	return { layer, opens: () => opens };
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-erasure-dispatch-fence-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function command<const Payload extends AuthoritativeCommandEnvelope["payload"]>(
	thread_id: string,
	message_id: string,
	payload: Payload,
): Omit<AuthoritativeCommandEnvelope, "payload"> & { readonly payload: Payload } {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
		thread_id,
	};
}

function graph_start(thread_id: string, assignment_count = 1) {
	return command(thread_id, "graph_start", {
		assignments: Array.from({ length: assignment_count }, (_, index) => ({
			assignment_id: `assignment_graph_${index + 1}`,
			engine_id: "fenced",
			expected_result: "Return a result",
			instructions: `Perform graph work ${index + 1}`,
			parent_node_id: "group_graph",
			permission_policy: {
				approval: "on_request",
				network_access: false,
				write_access: true,
			},
			profile: "default",
			role: `implementer_${index + 1}`,
			scope: { kind: "files", value: "src", write_access: true },
			summary_contract: "Return a concise summary",
			workspace: {
				isolation: "isolated",
				workspace_id: `workspace_graph_${index + 1}`,
				working_directory: tmpdir(),
			},
		})),
		group_id: "group_graph",
		max_concurrency: 4,
		name_bank: ["Bop"],
		type: "orchestration.group.start",
	});
}

function terminal_open(thread_id: string, message_id: string, terminal_id: string) {
	return command(thread_id, message_id, {
		args: [],
		cols: 100,
		executable: "test-shell",
		rows: 30,
		terminal_id,
		type: "terminal.open",
		working_directory: tmpdir(),
		workspace_id: "workspace_terminal",
	});
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread erasure dispatch fence", () => {
	it("excludes claimed ordinary and graph work from pending, claim, and restart dispatch", async () => {
		const database_path = await make_database_path();
		const first_engine = make_counting_engine("fenced");
		const first_runtime = make_backend_runtime({
			database_path,
			engines: [first_engine.engine],
			migrations_path,
		});

		try {
			const result = await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const graph = yield* AgentGraphRepository;
					const ordinary = yield* OrchestrationRepository;
					const router = yield* ProtocolRouter;
					const thread_id = "thread_persisted_claim";

					yield* router.Route(
						command(thread_id, "thread_create", {
							title: "Persisted claim",
							type: "thread.create",
						}),
					);
					const accepted = yield* ordinary.Accept(
						command(thread_id, "ordinary_send", {
							engine_id: "fenced",
							text: "Perform ordinary work",
							type: "thread.send_message",
							working_directory: tmpdir(),
						}),
						false,
					);
					yield* graph.StartGroup(graph_start(thread_id));

					const ordinary_before = yield* ordinary.GetPending();
					const graph_before = yield* graph.GetPendingRuns();

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-10T18:00:00.000Z",
						thread_id,
					});

					const ordinary_after = yield* ordinary.GetPending();
					const graph_after = yield* graph.GetPendingRuns();
					const ordinary_claimed = yield* ordinary.ClaimOutbox(
						ordinary_before[0]!.command_id,
					);
					const graph_claimed = yield* Effect.forEach(graph_before, (work) =>
						graph.ClaimRun(work.run_id, "blocked_instance"),
					);

					return {
						accepted,
						graph_after,
						graph_before,
						graph_claimed,
						ordinary_after,
						ordinary_before,
						ordinary_claimed,
					};
				}),
			);

			expect(result.accepted.run_id).not.toBe("unknown");
			expect(result.ordinary_before).toHaveLength(1);
			expect(result.graph_before).toHaveLength(1);
			expect(result.ordinary_after).toEqual([]);
			expect(result.graph_after).toEqual([]);
			expect(result.ordinary_claimed).toBe(false);
			expect(result.graph_claimed).toEqual([false]);
			expect(first_engine.opens()).toBe(0);
		} finally {
			await first_runtime.dispose();
		}

		const restart_engine = make_counting_engine("fenced");
		const restart_runtime = make_backend_runtime({
			database_path,
			engines: [restart_engine.engine],
			migrations_path,
		});

		try {
			const restarted = await restart_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const threads = yield* ThreadReadModel;

					return {
						snapshot: yield* threads.Snapshot(),
						tombstones: yield* database.client.select().from(ThreadTombstones),
					};
				}),
			);

			expect(restart_engine.opens()).toBe(0);
			expect(restarted.snapshot.threads).toEqual([]);
			expect(restarted.tombstones).toMatchObject([{ thread_id: "thread_persisted_claim" }]);
		} finally {
			await restart_runtime.dispose();
		}
	});

	it("keeps graph fan-out concurrent while quiesce drains blocked starts before erasure", async () => {
		const database_path = await make_database_path();
		const blocking = await make_blocking_engine("fenced");
		const runtime = make_backend_runtime({
			database_path,
			engines: [blocking.engine],
			migrations_path,
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const graph = yield* AgentGraphOrchestrator;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;
					const thread_id = "thread_graph_race";

					yield* router.Route(
						command(thread_id, "graph_thread_create", {
							title: "Graph race",
							type: "thread.create",
						}),
					);
					yield* graph.Handle(graph_start(thread_id, 3));
					yield* Effect.forEach([0, 1, 2], () => Queue.take(blocking.open_started), {
						discard: true,
					});

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-10T18:00:00.000Z",
						thread_id,
					});
					const quiesce = yield* graph
						.QuiesceThread(thread_id)
						.pipe(Effect.forkChild({ startImmediately: true }));

					yield* Effect.yieldNow;
					const completed_before_release = quiesce.pollUnsafe() !== undefined;

					yield* Deferred.succeed(blocking.open_release, undefined);
					yield* Fiber.join(quiesce);
					yield* Effect.forEach([0, 1, 2], () => Queue.take(blocking.scope_closed), {
						discard: true,
					});

					const erased = yield* erasure.ResumeClaimed("2026-07-10T18:00:00.000Z");

					return {
						completed_before_release,
						erased,
						snapshot: yield* threads.Snapshot(),
					};
				}),
			);

			expect(result.completed_before_release).toBe(false);
			expect(blocking.max_active_opens()).toBe(3);
			expect(blocking.active_opens()).toBe(0);
			expect(blocking.active_runs()).toBe(0);
			expect(result.erased).toEqual(["thread_graph_race"]);
			expect(result.snapshot.threads).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("waits for an active ordinary outbox send before closing and erasing its run", async () => {
		const database_path = await make_database_path();
		const blocking = await make_blocking_engine("fenced");
		const runtime = make_backend_runtime({
			database_path,
			engines: [blocking.engine],
			migrations_path,
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const orchestrator = yield* AgentOrchestrator;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;
					const thread_id = "thread_ordinary_send_race";

					yield* router.Route(
						command(thread_id, "ordinary_thread_create", {
							title: "Ordinary send race",
							type: "thread.create",
						}),
					);
					const accepted = yield* orchestrator.Handle(
						command(thread_id, "ordinary_start", {
							engine_id: "fenced",
							text: "Start ordinary work",
							type: "thread.send_message",
							working_directory: tmpdir(),
						}),
					);

					yield* Queue.take(blocking.open_started);
					yield* Deferred.succeed(blocking.open_release, undefined);
					yield* Queue.take(blocking.observer_started);
					yield* orchestrator.Handle({
						...command(thread_id, "ordinary_cancel", { type: "run.cancel" }),
						run_id: accepted.run_id,
					});
					yield* Queue.take(blocking.send_started);

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-10T18:00:00.000Z",
						thread_id,
					});
					const quiesce = yield* orchestrator
						.QuiesceThread(thread_id)
						.pipe(Effect.forkChild({ startImmediately: true }));

					yield* Effect.yieldNow;
					const completed_before_release = quiesce.pollUnsafe() !== undefined;

					yield* Deferred.succeed(blocking.send_release, undefined);
					yield* Fiber.join(quiesce);
					yield* Queue.take(blocking.scope_closed);

					const erased = yield* erasure.ResumeClaimed("2026-07-10T18:00:00.000Z");

					return {
						completed_before_release,
						erased,
						snapshot: yield* threads.Snapshot(),
					};
				}),
			);

			expect(result.completed_before_release).toBe(false);
			expect(blocking.active_sends()).toBe(0);
			expect(blocking.active_runs()).toBe(0);
			expect(result.erased).toEqual(["thread_ordinary_send_race"]);
			expect(result.snapshot.threads).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("waits for an active graph control send before closing and erasing its run", async () => {
		const database_path = await make_database_path();
		const blocking = await make_blocking_engine("fenced");
		const runtime = make_backend_runtime({
			database_path,
			engines: [blocking.engine],
			migrations_path,
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const graph = yield* AgentGraphOrchestrator;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;
					const thread_id = "thread_graph_send_race";

					yield* router.Route(
						command(thread_id, "graph_send_thread_create", {
							title: "Graph send race",
							type: "thread.create",
						}),
					);
					yield* graph.Handle(graph_start(thread_id));
					yield* Queue.take(blocking.open_started);
					yield* Deferred.succeed(blocking.open_release, undefined);
					yield* Queue.take(blocking.observer_started);

					const control = yield* graph
						.Handle(
							command(thread_id, "graph_stop", {
								assignment_id: "assignment_graph_1",
								group_id: "group_graph",
								type: "assignment.stop",
							}),
						)
						.pipe(Effect.forkChild({ startImmediately: true }));

					yield* Queue.take(blocking.send_started);
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-10T18:00:00.000Z",
						thread_id,
					});
					const quiesce = yield* graph
						.QuiesceThread(thread_id)
						.pipe(Effect.forkChild({ startImmediately: true }));

					yield* Effect.yieldNow;
					const completed_before_release = quiesce.pollUnsafe() !== undefined;

					yield* Deferred.succeed(blocking.send_release, undefined);
					yield* Fiber.await(control);
					yield* Fiber.join(quiesce);
					yield* Queue.take(blocking.scope_closed);
					const quiesced_error = yield* graph
						.Handle(
							command(thread_id, "graph_stop_after_quiesce", {
								assignment_id: "assignment_graph_1",
								group_id: "group_graph",
								type: "assignment.stop",
							}),
						)
						.pipe(Effect.flip);
					const quiesced_claims = (yield* database.client
						.select()
						.from(OrchestrationGraphCommands)).filter(
						({ message_id }) => message_id === "graph_stop_after_quiesce",
					);

					const erased = yield* erasure.ResumeClaimed("2026-07-10T18:00:00.000Z");

					return {
						completed_before_release,
						erased,
						quiesced_claims,
						quiesced_error,
						snapshot: yield* threads.Snapshot(),
					};
				}),
			);

			expect(result.completed_before_release).toBe(false);
			expect(blocking.active_sends()).toBe(0);
			expect(blocking.active_runs()).toBe(0);
			expect(result.quiesced_error).toBeInstanceOf(AgentGraphInvalid);
			expect(result.quiesced_claims).toEqual([]);
			expect(result.erased).toEqual(["thread_graph_send_race"]);
			expect(result.snapshot.threads).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects terminal opens after quiesce or a durable claim before driver side effects", async () => {
		const terminal_driver = make_terminal_driver();
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
			terminal_driver: terminal_driver.layer,
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const router = yield* ProtocolRouter;
					const terminals = yield* TerminalSessionService;
					const quiesced_thread = "thread_terminal_quiesced";
					const claimed_thread = "thread_terminal_claimed";

					yield* router.Route(
						command(quiesced_thread, "quiesced_thread_create", {
							title: "Quiesced terminal",
							type: "thread.create",
						}),
					);
					yield* router.Route(
						command(claimed_thread, "claimed_thread_create", {
							title: "Claimed terminal",
							type: "thread.create",
						}),
					);

					yield* terminals.QuiesceThread(quiesced_thread);
					const quiesced_error = yield* terminals
						.Handle(
							terminal_open(quiesced_thread, "terminal_after_quiesce", "terminal_1"),
						)
						.pipe(Effect.flip);

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-10T18:00:00.000Z",
						thread_id: claimed_thread,
					});
					const claimed_error = yield* terminals
						.Handle(terminal_open(claimed_thread, "terminal_after_claim", "terminal_2"))
						.pipe(Effect.flip);

					return {
						claimed_error,
						commands: yield* database.client.select().from(TerminalCommands),
						quiesced_error,
						sessions: yield* database.client.select().from(TerminalSessions),
					};
				}),
			);

			expect(result.quiesced_error).toBeInstanceOf(TerminalNotFound);
			expect(result.claimed_error).toBeInstanceOf(TerminalNotFound);
			expect(terminal_driver.opens()).toBe(0);
			expect(result.commands).toEqual([]);
			expect(result.sessions).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("erases durable preview targets, inspections, and commands with their thread", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			engines: [make_counting_engine("preview_erase").engine],
			migrations_path,
		});
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const router = yield* ProtocolRouter;
					const thread_id = "thread_preview_erase";
					yield* router.Route(
						command(thread_id, "preview_erase_create", {
							title: "Preview erase",
							type: "thread.create",
						}),
					);
					yield* database.client.insert(PreviewTargets).values({
						created_at: "2026-07-18T20:00:00.000Z",
						health_json: null,
						journal_sequence: 1,
						last_error: null,
						launch_state: "idle",
						port: 5173,
						project_id: "project",
						removed_at: null,
						routes_json: "[]",
						source_id: null,
						source_kind: null,
						state: "registered",
						target_id: "target_erase",
						thread_id,
						updated_at: "2026-07-18T20:00:00.000Z",
						url: "http://localhost:5173/",
						workspace_id: "workspace",
					});
					yield* database.client.insert(PreviewInspectionSessions).values({
						closed_at: null,
						connector_id: "connector",
						journal_sequence: 1,
						last_error: null,
						opened_at: "2026-07-18T20:00:00.000Z",
						reconnect_state: "connected",
						session_id: "session_erase",
						state: "open",
						target_id: "target_erase",
						thread_id,
						updated_at: "2026-07-18T20:00:00.000Z",
					});
					yield* database.client.insert(PreviewCommands).values({
						action: "register",
						created_at: "2026-07-18T20:00:00.000Z",
						journal_sequence: 1,
						message_id: "preview_erase_command",
						payload_json: "{}",
						thread_id,
					});
					yield* database.client
						.insert(ThreadErasureClaims)
						.values({ claimed_at: "2026-07-18T20:01:00.000Z", thread_id });
					yield* erasure.ResumeClaimed("2026-07-18T20:02:00.000Z");
					return {
						commands: yield* database.client.select().from(PreviewCommands),
						inspections: yield* database.client
							.select()
							.from(PreviewInspectionSessions),
						targets: yield* database.client.select().from(PreviewTargets),
					};
				}),
			);
			expect(result).toEqual({ commands: [], inspections: [], targets: [] });
		} finally {
			await runtime.dispose();
		}
	});

	it("releases an erasure claim without deleting a thread while its preview side effect lease is live", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			engines: [make_counting_engine("preview_lease_fence").engine],
			migrations_path,
		});
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const router = yield* ProtocolRouter;
					const thread_id = "thread_preview_active_lease";
					yield* router.Route(
						command(thread_id, "preview_active_lease_create", {
							title: "Preview active lease",
							type: "thread.create",
						}),
					);
					yield* database.client.insert(PreviewDispatchLeases).values({
						acquired_at: "2026-07-18T20:00:00.000Z",
						expires_at: "2026-07-18T20:10:00.000Z",
						kind: "launch",
						lease_id: "active_preview_lease",
						owner_instance_id: "other_runtime",
						session_id: null,
						target_id: null,
						thread_id,
					});
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-18T20:01:00.000Z",
						thread_id,
					});
					const erased = yield* erasure.ResumeClaimed("2026-07-18T20:02:00.000Z");
					return {
						erased,
						claims: yield* database.client.select().from(ThreadErasureClaims),
						threads: yield* database.client.select().from(Threads),
					};
				}),
			);
			expect(result.erased).toEqual([]);
			expect(result.claims).toEqual([]);
			expect(result.threads.map((thread) => thread.thread_id)).toContain(
				"thread_preview_active_lease",
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects an unrelated raw journal event during a claimed preview lease", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			engines: [make_counting_engine("preview_trigger_fence").engine],
			migrations_path,
		});
		try {
			const rejected = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const router = yield* ProtocolRouter;
					const thread_id = "thread_preview_trigger_fence";
					yield* router.Route(
						command(thread_id, "preview_trigger_fence_create", {
							title: "Preview trigger fence",
							type: "thread.create",
						}),
					);
					yield* database.client.insert(PreviewDispatchLeases).values({
						acquired_at: "2026-07-18T20:00:00.000Z",
						expires_at: "2026-07-18T20:10:00.000Z",
						kind: "launch",
						lease_id: "preview_trigger_live_lease",
						owner_instance_id: "other_runtime",
						session_id: null,
						target_id: "preview_trigger_target",
						thread_id,
					});
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-18T20:01:00.000Z",
						thread_id,
					});

					return yield* Effect.exit(
						database.client.insert(JournalEvents).values({
							agent_id: null,
							causation_id: "unrelated_raw_causation",
							correlation_id: "unrelated_raw_correlation",
							event_id: "unrelated_raw_event",
							event_type: "unrelated.raw.event",
							occurred_at: "2026-07-18T20:02:00.000Z",
							origin: "backend",
							payload_json: '{"type":"unrelated.raw.event"}',
							raw_origin_json: null,
							run_id: null,
							schema_version: 1,
							stream_id: `thread:${thread_id}`,
							stream_sequence: 2,
							thread_id,
						}),
					);
				}),
			);

			expect(rejected._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});
});

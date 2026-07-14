import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Cause, Deferred, Effect, Queue, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	Engine,
	EngineCapabilityName,
	EngineCommand,
	EngineObservation,
	EngineOpenInput,
	EngineRun,
	EngineRunTerminalState,
} from "@artisan/engines";
import type { CommandEnvelope, OrchestrationGraph } from "@artisan/protocol";
import { AgentGraphOrchestrator, make_backend_runtime, ProtocolRouter } from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

interface FakeRun {
	readonly closed: Deferred.Deferred<EngineRunTerminalState>;
	readonly commands: Array<EngineCommand>;
	readonly input: EngineOpenInput;
	readonly queue: Queue.Queue<EngineObservation, Cause.Done<void>>;
	sequence: number;
}

function make_capabilities(
	overrides: Partial<Record<EngineCapabilityName, "supported" | "unsupported">> = {},
) {
	return Object.fromEntries(
		[
			"approval",
			"auth",
			"cancel",
			"close",
			"events",
			"global_guidance",
			"harness_context",
			"model_selection",
			"native_tools",
			"probe",
			"question",
			"raw_frames",
			"resume",
			"start",
			"steer",
			"subagents",
		].map((name) => [name, { state: overrides[name as EngineCapabilityName] ?? "supported" }]),
	) as Engine["Descriptor"]["capabilities"];
}

function make_graph_engine(
	id: string,
	overrides: Partial<Record<EngineCapabilityName, "supported" | "unsupported">> = {},
	reject_commands = false,
) {
	const runs: Array<FakeRun> = [];
	let active = 0;
	let max_active = 0;
	let scopes_closed = 0;

	const Open = (input: EngineOpenInput) =>
		Effect.gen(function* () {
			const queue = yield* Queue.unbounded<EngineObservation, Cause.Done<void>>();
			const closed = yield* Deferred.make<EngineRunTerminalState>();
			const commands: Array<EngineCommand> = [];
			const fake = { closed, commands, input, queue, sequence: 0 } satisfies FakeRun;

			active += 1;
			max_active = Math.max(max_active, active);
			runs.push(fake);

			yield* Effect.addFinalizer(() =>
				Effect.gen(function* () {
					active -= 1;
					scopes_closed += 1;
					yield* Queue.end(queue);
					yield* Deferred.succeed(closed, "closed");
				}),
			);

			return {
				artisan_run_id: input.artisan_run_id,
				Closed: Deferred.await(closed),
				Events: Stream.fromQueue(queue),
				native_thread_id: `native:${input.artisan_run_id}`,
				resume_token: { native_thread_id: `native:${input.artisan_run_id}` },
				Send: (command) =>
					Effect.sync(() => void fake.commands.push(command)).pipe(
						Effect.andThen(
							reject_commands
								? Effect.die("The fake engine rejected the command")
								: Effect.void,
						),
					),
			} satisfies EngineRun;
		});

	const Emit = (
		index: number,
		observation: { readonly _tag: "run_terminal"; readonly state: EngineRunTerminalState },
	) =>
		Effect.gen(function* () {
			const run = runs[index]!;

			run.sequence += 1;
			yield* Queue.offer(run.queue, {
				...observation,
				artisan_run_id: run.input.artisan_run_id,
				observation_id: `${id}:${index}:${run.sequence}`,
				raw: {
					engine_id: id,
					frame: observation,
					native_id: `${id}:${index}:${run.sequence}`,
					transport: "test",
				},
				sequence: run.sequence,
			} as EngineObservation);
		});

	const Finish = (index: number, state: EngineRunTerminalState) =>
		Effect.gen(function* () {
			yield* Emit(index, { _tag: "run_terminal", state });
			yield* Queue.end(runs[index]!.queue);
			yield* Deferred.succeed(runs[index]!.closed, state);
		});

	return {
		Emit,
		Finish,
		engine: {
			Descriptor: {
				capabilities: make_capabilities(overrides),
				display_name: `Graph engine ${id}`,
				id,
				transport: "test",
			},
			Open,
			Probe: () => Effect.die("Probe is not used by graph tests"),
		} satisfies Engine,
		max_active: () => max_active,
		runs,
		scopes_closed: () => scopes_closed,
	};
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-agent-graph-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function command(message_id: string, payload: CommandEnvelope["payload"]): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		thread_id: "thread_graph",
	};
}

function assignment(assignment_id: string, engine_id: string, role: string, display_name?: string) {
	return {
		assignment_id,
		...(display_name ? { display_name } : {}),
		engine_id,
		expected_result: `${role} result`,
		instructions: `Perform ${role} work`,
		parent_node_id: "group_graph",
		permission_policy: {
			approval: "on_request" as const,
			metadata: { policy: role },
			network_access: role === "researcher",
			write_access: role === "implementer",
		},
		profile: `${role}-profile`,
		role,
		scope: {
			kind: "files" as const,
			value: `src/${assignment_id}.ts`,
			write_access: role === "implementer",
		},
		summary_contract: `Return a ${role} summary`,
		workspace: {
			isolation: "isolated" as const,
			workspace_id: `workspace_${assignment_id}`,
			working_directory: tmpdir(),
		},
	};
}

function start_command(
	message_id: string,
	assignments: ReadonlyArray<ReturnType<typeof assignment>>,
	name_bank: readonly [string, ...string[]] = ["Bop"],
	max_concurrency = 4,
) {
	return command(message_id, {
		assignments: [...assignments],
		group_id: "group_graph",
		max_concurrency,
		name_bank,
		type: "orchestration.group.start",
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

function get_graph(runtime: ReturnType<typeof make_backend_runtime>) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const graph = yield* AgentGraphOrchestrator;

			return yield* graph.GetGraph("group_graph");
		}),
	);
}

async function wait_for_graph(
	runtime: ReturnType<typeof make_backend_runtime>,
	predicate: (graph: OrchestrationGraph) => boolean,
) {
	for (let attempt = 0; attempt < 200; attempt += 1) {
		const graph = await get_graph(runtime);

		if (predicate(graph)) {
			return graph;
		}

		await new Promise<void>((resolve) => setTimeout(resolve, 10));
	}

	throw new Error("Graph did not reach the expected state");
}

async function create_thread(runtime: ReturnType<typeof make_backend_runtime>) {
	await route(
		runtime,
		command("create_graph_thread", { title: "Agent graph", type: "thread.create" }),
	);
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("durable multi-agent graph", () => {
	it("runs assignments concurrently and preserves identity, role, scope, permissions, and names", async () => {
		const database_path = await make_database_path();
		const fake = make_graph_engine("graph");
		const runtime = make_backend_runtime({
			database_path,
			engines: [fake.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			await route(
				runtime,
				start_command(
					"start_graph",
					[
						assignment("assignment_a", "graph", "researcher"),
						assignment("assignment_b", "graph", "implementer"),
						assignment("assignment_c", "graph", "tester"),
					],
					["Bop", "bop"],
				),
			);

			const graph = await wait_for_graph(
				runtime,
				(current) =>
					current.agent_runs.filter(({ state }) => state === "running").length === 3,
			);
			const workers = graph.agent_instances.filter(({ role }) => role !== "coordinator");

			expect(fake.max_active()).toBeGreaterThanOrEqual(2);
			expect(new Set(workers.map(({ display_name }) => display_name)).size).toBe(3);
			expect(workers.map(({ display_name }) => display_name)).toEqual([
				"Bop",
				"bop 2",
				"Tester 2",
			]);
			expect(graph.assignments[1]).toMatchObject({
				permission_policy: {
					approval: "on_request",
					metadata: { policy: "implementer" },
					write_access: true,
				},
				profile: "implementer-profile",
				role: "implementer",
				scope: { kind: "files", value: "src/assignment_b.ts", write_access: true },
				workspace: { isolation: "isolated", workspace_id: "workspace_assignment_b" },
			});
			expect(fake.runs.map(({ input }) => input.permission_policy)).toEqual(
				expect.arrayContaining([
					{
						approval: "on_request",
						network_access: true,
						write_access: false,
					},
					{
						approval: "on_request",
						network_access: false,
						write_access: true,
					},
					{
						approval: "on_request",
						network_access: false,
						write_access: false,
					},
				]),
			);
			expect(
				graph.agent_runs.map(({ native_identity, native_thread_id }) => ({
					native_identity,
					native_thread_id,
				})),
			).toEqual(
				graph.agent_runs.map(({ native_thread_id }) => ({
					native_identity: { thread_id: native_thread_id },
					native_thread_id,
				})),
			);

			const renamed = await route(
				runtime,
				command("rename_agent", {
					agent_id: workers[1]!.agent_id,
					display_name: "Tinker",
					group_id: "group_graph",
					type: "agent_instance.rename",
				}),
			);

			expect(renamed).toMatchObject([
				{ payload: { status: "accepted" } },
				{ payload: { display_name: "Tinker", type: "agent_instance.renamed" } },
			]);
			expect((await get_graph(runtime)).agent_instances).toContainEqual(
				expect.objectContaining({ agent_id: workers[1]!.agent_id, display_name: "Tinker" }),
			);

			const collision = await route(
				runtime,
				command("rename_collision", {
					agent_id: workers[0]!.agent_id,
					display_name: "tInKeR",
					group_id: "group_graph",
					type: "agent_instance.rename",
				}),
			);

			expect(collision).toMatchObject([
				{ payload: { error: { code: "orchestration.graph_invalid" }, status: "rejected" } },
			]);
			expect(
				(await get_graph(runtime)).agent_instances.filter(
					({ display_name }) => display_name === "Tinker",
				),
			).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}

		expect(fake.scopes_closed()).toBe(3);
	});

	it("projects safe heartbeats and never persists private reasoning text", async () => {
		const database_path = await make_database_path();
		const fake = make_graph_engine("graph");
		const runtime = make_backend_runtime({
			database_path,
			engines: [fake.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			await route(
				runtime,
				start_command("start_heartbeat", [
					assignment("assignment_a", "graph", "researcher"),
					assignment("assignment_b", "graph", "tester"),
				]),
			);
			await wait_for_graph(runtime, (graph) =>
				graph.agent_runs.every(({ state }) => state === "running"),
			);

			await route(
				runtime,
				command("heartbeat_safe", {
					assignment_id: "assignment_a",
					blocked_reason: "Waiting for deterministic test output",
					confidence: 0.7,
					current_action: "Comparing adapter behavior",
					group_id: "group_graph",
					short_description: "x".repeat(300),
					type: "assignment.heartbeat",
					updated_at: "2026-07-10T08:01:00.000Z",
				}),
			);

			const blocked = await get_graph(runtime);
			const heartbeat = blocked.assignments[0]!.heartbeat!;

			expect(blocked.assignments[0]!.state).toBe("blocked");
			expect(heartbeat.short_description).toHaveLength(160);
			expect(heartbeat.current_action).toBe("Comparing adapter behavior");

			const clear_heartbeat = command("heartbeat_cleared", {
				assignment_id: "assignment_a",
				confidence: 0.9,
				current_action: "Applying the verified result",
				group_id: "group_graph",
				short_description: "Unblocked",
				type: "assignment.heartbeat",
				updated_at: "2026-07-10T08:01:30.000Z",
			});
			const clear_accepted = await route(runtime, clear_heartbeat);
			const clear_duplicate = await route(runtime, clear_heartbeat);

			const cleared = await get_graph(runtime);
			const cleared_assignment = cleared.assignments.find(
				({ assignment_id }) => assignment_id === "assignment_a",
			)!;

			expect(cleared_assignment.state).toBe("running");
			expect(cleared_assignment.heartbeat?.blocked_reason).toBeUndefined();
			expect(clear_accepted[0]).toMatchObject({ payload: { status: "accepted" } });
			expect(clear_duplicate[0]).toMatchObject({ payload: { status: "duplicate" } });
			expect(clear_duplicate.slice(1)).toEqual(clear_accepted.slice(1));

			const stale = await route(
				runtime,
				command("heartbeat_stale", {
					assignment_id: "assignment_a",
					confidence: 0.1,
					current_action: "Delivering stale status",
					group_id: "group_graph",
					short_description: "Stale",
					type: "assignment.heartbeat",
					updated_at: "2026-07-10T08:01:00.000Z",
				}),
			);
			const equal_time = await route(
				runtime,
				command("heartbeat_equal_time", {
					assignment_id: "assignment_a",
					confidence: 0.3,
					current_action: "Delivering equal timestamp status",
					group_id: "group_graph",
					short_description: "Equal timestamp",
					type: "assignment.heartbeat",
					updated_at: "2026-07-10T08:01:30.000Z",
				}),
			);

			expect(stale).toMatchObject([
				{ payload: { error: { code: "orchestration.graph_invalid" }, status: "rejected" } },
			]);
			expect(equal_time).toMatchObject([
				{ payload: { error: { code: "orchestration.graph_invalid" }, status: "rejected" } },
			]);
			expect(
				(await get_graph(runtime)).assignments.find(
					({ assignment_id }) => assignment_id === "assignment_a",
				)?.heartbeat,
			).toEqual(cleared_assignment.heartbeat);

			const unsafe = await route(
				runtime,
				command("heartbeat_unsafe", {
					assignment_id: "assignment_a",
					confidence: 0.2,
					current_action: "Writing my chain-of-thought for inspection",
					group_id: "group_graph",
					short_description: "Inspecting adapters",
					type: "assignment.heartbeat",
					updated_at: "2026-07-10T08:02:00.000Z",
				}),
			);

			expect(unsafe).toMatchObject([
				{ payload: { error: { code: "orchestration.graph_invalid" }, status: "rejected" } },
			]);
			expect(JSON.stringify(await get_graph(runtime))).not.toContain("chain-of-thought");
		} finally {
			await runtime.dispose();
		}
	});

	it("normalizes visible graph labels and rejects oversized or controlled input", async () => {
		const database_path = await make_database_path();
		const fake = make_graph_engine("graph");
		const runtime = make_backend_runtime({
			database_path,
			engines: [fake.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			const base_assignments = [
				assignment("assignment_a", "graph", "researcher"),
				assignment("assignment_b", "graph", "tester"),
			] as const;
			const oversized_role = await route(
				runtime,
				start_command("invalid_role", [
					{ ...base_assignments[0], role: "r".repeat(65) },
					base_assignments[1],
				]),
			);
			const oversized_name = await route(
				runtime,
				start_command("invalid_name_bank", base_assignments, ["n".repeat(65)]),
			);

			expect(oversized_role).toMatchObject([
				{ kind: "protocol.error", payload: { code: "protocol.invalid_message" } },
			]);
			expect(oversized_name).toMatchObject([
				{ kind: "protocol.error", payload: { code: "protocol.invalid_message" } },
			]);

			await route(
				runtime,
				start_command("start_normalized_labels", [
					{
						...base_assignments[0],
						display_name: "  Nova   Prime  ",
						role: "  build   lead  ",
					},
					base_assignments[1],
				]),
			);
			const graph = await wait_for_graph(runtime, (projection) =>
				projection.agent_runs.every(({ state }) => state === "running"),
			);
			const normalized_assignment = graph.assignments.find(
				({ assignment_id }) => assignment_id === "assignment_a",
			)!;
			const normalized_agent = graph.agent_instances.find(
				({ agent_id }) => agent_id === normalized_assignment.agent_id,
			)!;

			expect(normalized_assignment.role).toBe("build lead");
			expect(normalized_agent).toMatchObject({
				display_name: "Nova Prime",
				role: "build lead",
			});

			const controlled_name = await route(
				runtime,
				command("rename_control_character", {
					agent_id: normalized_agent.agent_id,
					display_name: "Broken\nName",
					group_id: "group_graph",
					type: "agent_instance.rename",
				}),
			);
			const controlled_status = await route(
				runtime,
				command("heartbeat_control_character", {
					assignment_id: "assignment_a",
					confidence: 0.5,
					current_action: "Broken\nstatus",
					group_id: "group_graph",
					short_description: "Status",
					type: "assignment.heartbeat",
					updated_at: "2026-07-10T08:03:00.000Z",
				}),
			);

			expect(controlled_name).toMatchObject([
				{ kind: "protocol.error", payload: { code: "protocol.invalid_message" } },
			]);
			expect(controlled_status).toMatchObject([
				{ kind: "protocol.error", payload: { code: "protocol.invalid_message" } },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("enforces the durable per-group concurrency bound", async () => {
		const database_path = await make_database_path();
		const fake = make_graph_engine("graph");
		const runtime = make_backend_runtime({
			database_path,
			engines: [fake.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			await route(
				runtime,
				start_command(
					"start_bounded",
					[
						assignment("assignment_a", "graph", "researcher"),
						assignment("assignment_b", "graph", "implementer"),
						assignment("assignment_c", "graph", "tester"),
					],
					["Bop"],
					2,
				),
			);
			const bounded = await wait_for_graph(
				runtime,
				(graph) => graph.agent_runs.filter(({ state }) => state === "running").length === 2,
			);

			expect(fake.runs).toHaveLength(2);
			expect(bounded.agent_runs.filter(({ state }) => state === "queued")).toHaveLength(1);
			expect(fake.max_active()).toBe(2);

			await Effect.runPromise(fake.Finish(0, "completed"));
			await wait_for_graph(
				runtime,
				(graph) =>
					graph.agent_runs.filter(({ state }) => state === "running").length === 2 &&
					graph.agent_runs.some(({ state }) => state === "complete"),
			);

			expect(fake.runs).toHaveLength(3);
			expect(fake.max_active()).toBe(2);
		} finally {
			await runtime.dispose();
		}

		expect(fake.scopes_closed()).toBe(3);
	});

	it("handles control capability outcomes and exact command idempotency", async () => {
		const database_path = await make_database_path();
		const supported = make_graph_engine("supported");
		const unsupported = make_graph_engine("unsupported", {
			cancel: "unsupported",
			close: "unsupported",
			steer: "unsupported",
		});
		const rejected = make_graph_engine("rejected", {}, true);
		const runtime = make_backend_runtime({
			database_path,
			engines: [supported.engine, unsupported.engine, rejected.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			await route(
				runtime,
				start_command("start_controls", [
					assignment("assignment_a", "supported", "implementer"),
					assignment("assignment_b", "unsupported", "reviewer"),
					assignment("assignment_c", "rejected", "tester"),
				]),
			);
			await wait_for_graph(runtime, (graph) =>
				graph.agent_runs.every(({ state }) => state === "running"),
			);

			const steer = command("steer_once", {
				assignment_id: "assignment_a",
				group_id: "group_graph",
				text: "Inspect the durable boundary",
				type: "assignment.steer",
			});

			await route(runtime, steer);
			await route(runtime, steer);

			expect(supported.runs[0]!.commands).toEqual([
				{ _tag: "steer", command_id: "steer_once", text: "Inspect the durable boundary" },
			]);

			const unsupported_result = await route(
				runtime,
				command("steer_unsupported", {
					assignment_id: "assignment_b",
					group_id: "group_graph",
					text: "Try steering",
					type: "assignment.steer",
				}),
			);
			const pause_result = await route(
				runtime,
				command("pause_unsupported", {
					assignment_id: "assignment_a",
					group_id: "group_graph",
					type: "assignment.pause",
				}),
			);
			const resume_result = await route(
				runtime,
				command("resume_unsupported", {
					assignment_id: "assignment_a",
					group_id: "group_graph",
					type: "assignment.resume",
				}),
			);
			const stop_unsupported = await route(
				runtime,
				command("stop_unsupported", {
					assignment_id: "assignment_b",
					group_id: "group_graph",
					type: "assignment.stop",
				}),
			);
			const rejected_result = await route(
				runtime,
				command("steer_rejected", {
					assignment_id: "assignment_c",
					group_id: "group_graph",
					text: "Reject this control",
					type: "assignment.steer",
				}),
			);
			const cross_thread = await route(runtime, {
				...command("cross_thread_control", {
					assignment_id: "assignment_a",
					group_id: "group_graph",
					text: "Do not disclose the group",
					type: "assignment.steer",
				}),
				thread_id: "thread_other",
			});

			expect(unsupported_result).toMatchObject([
				{ payload: { status: "accepted" } },
				{ payload: { action: "steer", outcome: "unsupported" } },
			]);
			expect(pause_result).toMatchObject([
				{ payload: { status: "accepted" } },
				{ payload: { action: "pause", outcome: "unsupported" } },
			]);
			expect(resume_result).toMatchObject([
				{ payload: { status: "accepted" } },
				{ payload: { action: "resume", outcome: "unsupported" } },
			]);
			expect(stop_unsupported).toMatchObject([
				{ payload: { status: "accepted" } },
				{ payload: { action: "stop", outcome: "unsupported" } },
			]);
			expect(rejected_result).toMatchObject([
				{ payload: { status: "accepted" } },
				{ payload: { action: "steer", outcome: "rejected" } },
			]);
			expect(cross_thread).toMatchObject([
				{
					payload: {
						error: { code: "orchestration_group.not_found" },
						status: "rejected",
					},
				},
			]);

			await route(
				runtime,
				command("stop_supported", {
					assignment_id: "assignment_a",
					group_id: "group_graph",
					type: "assignment.stop",
				}),
			);

			expect(supported.runs[0]!.commands.at(-1)).toEqual({
				_tag: "cancel",
				command_id: "stop_supported",
			});

			await Effect.runPromise(supported.Finish(0, "cancelled"));
			const stopped = await wait_for_graph(
				runtime,
				(graph) =>
					graph.assignments.find(({ assignment_id }) => assignment_id === "assignment_a")
						?.state === "stopped",
			);

			expect(
				stopped.assignments.find(({ assignment_id }) => assignment_id === "assignment_a"),
			).toMatchObject({ state: "stopped" });
		} finally {
			await runtime.dispose();
		}
	});

	it("replays duplicate group starts and rejects changed intent atomically", async () => {
		const database_path = await make_database_path();
		const fake = make_graph_engine("graph");
		const runtime = make_backend_runtime({
			database_path,
			engines: [fake.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			const assignments = [
				assignment("assignment_a", "graph", "researcher"),
				assignment("assignment_b", "graph", "tester"),
			] as const;
			const start = start_command("start_idempotent", assignments);
			const accepted = await route(runtime, start);
			const duplicate = await route(runtime, start);
			const conflict = await route(
				runtime,
				start_command("start_idempotent", assignments, ["Bop"], 2),
			);
			const accepted_events = accepted.filter((envelope) => envelope.kind === "event");
			const receipt = accepted.find((envelope) => envelope.kind === "command.receipt");

			await wait_for_graph(runtime, (graph) =>
				graph.agent_runs.every(({ state }) => state === "running"),
			);

			expect(fake.runs).toHaveLength(2);
			expect(accepted[0]!.payload).toMatchObject({ status: "accepted" });
			expect(duplicate[0]!.payload).toMatchObject({ status: "duplicate" });
			expect(duplicate.slice(1)).toEqual(accepted.slice(1));
			expect(accepted_events.map(({ journal_sequence }) => journal_sequence)).toEqual(
				accepted_events
					.map(({ journal_sequence }) => journal_sequence)
					.sort((a, b) => a - b),
			);
			expect(accepted_events.map(({ sequence }) => sequence)).toEqual(
				accepted_events.map(({ sequence }) => sequence).sort((a, b) => a - b),
			);
			expect(receipt).toMatchObject({
				payload: { journal_sequence: accepted_events.at(-1)!.journal_sequence },
			});
			expect(conflict).toMatchObject([
				{ payload: { error: { code: "command.id_conflict" }, status: "rejected" } },
			]);
		} finally {
			await runtime.dispose();
		}
	});
});

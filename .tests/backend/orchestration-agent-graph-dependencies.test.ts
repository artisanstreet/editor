import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Cause, Deferred, Effect, Queue, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	Engine,
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

interface ControlledRun {
	readonly closed: Deferred.Deferred<EngineRunTerminalState>;
	readonly input: EngineOpenInput;
	readonly queue: Queue.Queue<EngineObservation, Cause.Done<void>>;
	sequence: number;
}

function make_dependency_engine() {
	const runs: Array<ControlledRun> = [];
	const capability_names = [
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
	] as const;
	const capabilities = Object.fromEntries(
		capability_names.map((name) => [name, { state: "supported" as const }]),
	) as Engine["Descriptor"]["capabilities"];
	const Open = (input: EngineOpenInput) =>
		Effect.gen(function* () {
			const queue = yield* Queue.unbounded<EngineObservation, Cause.Done<void>>();
			const closed = yield* Deferred.make<EngineRunTerminalState>();
			const controlled = { closed, input, queue, sequence: 0 } satisfies ControlledRun;

			runs.push(controlled);
			yield* Effect.addFinalizer(() =>
				Effect.gen(function* () {
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
				Send: (_command: EngineCommand) => Effect.void,
			} satisfies EngineRun;
		});
	const find_run = (assignment_id: string) =>
		runs.find((run) =>
			"initial_text" in run.input
				? run.input.initial_text.includes(`Work on ${assignment_id}`)
				: false,
		);
	const finish = (run: ControlledRun, state: EngineRunTerminalState) =>
		Effect.gen(function* () {
			run.sequence += 1;
			yield* Queue.offer(run.queue, {
				_tag: "run_terminal",
				artisan_run_id: run.input.artisan_run_id,
				observation_id: `terminal:${run.input.artisan_run_id}:${run.sequence}`,
				raw: {
					engine_id: "dependency",
					frame: { state },
					native_id: `native-terminal:${run.sequence}`,
					transport: "test",
				},
				sequence: run.sequence,
				state,
			});
			yield* Queue.end(run.queue);
			yield* Deferred.succeed(run.closed, state);
		});

	return {
		engine: {
			Descriptor: {
				capabilities,
				display_name: "Dependency graph engine",
				id: "dependency",
				transport: "test",
			},
			Open,
			Probe: () => Effect.die("Probe is not used by dependency tests"),
		} satisfies Engine,
		find_run,
		finish,
		runs,
	};
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-agent-graph-dependencies-"));

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
		sent_at: "2026-07-10T09:00:00.000Z",
		thread_id: "thread_dependencies",
	};
}

function assignment(assignment_id: string) {
	return {
		assignment_id,
		engine_id: "dependency",
		expected_result: `${assignment_id} result`,
		instructions: `Work on ${assignment_id}`,
		parent_node_id: "group_dependencies",
		permission_policy: {
			approval: "never" as const,
			network_access: false,
			write_access: false,
		},
		profile: "default",
		role: "worker",
		scope: { kind: "test" as const, value: assignment_id, write_access: false },
		summary_contract: "Return a concise result",
		workspace: {
			isolation: "isolated" as const,
			workspace_id: `workspace_${assignment_id}`,
			working_directory: tmpdir(),
		},
	};
}

function dependency(edge_id: string, from_node_id: string, to_node_id: string) {
	return { edge_id, from_node_id, kind: "dependency" as const, to_node_id };
}

function start_command(
	message_id: string,
	assignment_ids: ReadonlyArray<string>,
	edges: ReadonlyArray<ReturnType<typeof dependency>>,
) {
	return command(message_id, {
		assignments: assignment_ids.map(assignment),
		edges: [...edges],
		group_id: "group_dependencies",
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

			return yield* graph.GetGraph("group_dependencies");
		}),
	);
}

async function wait_for_graph(
	runtime: ReturnType<typeof make_backend_runtime>,
	predicate: (graph: OrchestrationGraph) => boolean,
) {
	for (let attempt = 0; attempt < 300; attempt += 1) {
		const graph = await get_graph(runtime);

		if (predicate(graph)) {
			return graph;
		}

		await new Promise<void>((resolve) => setTimeout(resolve, 10));
	}

	throw new Error("Dependency graph did not reach the expected state");
}

async function create_thread(runtime: ReturnType<typeof make_backend_runtime>) {
	await route(
		runtime,
		command("create_dependency_thread", {
			title: "Dependency graph",
			type: "thread.create",
		}),
	);
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("semantic orchestration graph dependencies", () => {
	it("opens a dependent assignment only after its predecessor completes", async () => {
		const controlled = make_dependency_engine();
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [controlled.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			await route(
				runtime,
				start_command(
					"start_ordered",
					["assignment_a", "assignment_b"],
					[dependency("depends_a_b", "assignment_a", "assignment_b")],
				),
			);
			const initial = await wait_for_graph(
				runtime,
				(graph) =>
					graph.assignments.find(({ assignment_id }) => assignment_id === "assignment_a")
						?.state === "running",
			);

			expect(controlled.runs).toHaveLength(1);
			expect(
				initial.assignments.find(({ assignment_id }) => assignment_id === "assignment_b"),
			).toMatchObject({ state: "blocked" });

			await Effect.runPromise(
				controlled.finish(controlled.find_run("assignment_a")!, "completed"),
			);
			const released = await wait_for_graph(
				runtime,
				(graph) =>
					graph.assignments.find(({ assignment_id }) => assignment_id === "assignment_b")
						?.state === "running",
			);

			expect(controlled.runs).toHaveLength(2);
			expect(released.edges).toContainEqual(
				expect.objectContaining({
					from_node_id: "assignment_a",
					kind: "dependency",
					to_node_id: "assignment_b",
				}),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("holds a fan-in assignment until every predecessor completes", async () => {
		const controlled = make_dependency_engine();
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [controlled.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			await route(
				runtime,
				start_command(
					"start_fan_in",
					["assignment_a", "assignment_b", "assignment_c"],
					[
						dependency("depends_a_c", "assignment_a", "assignment_c"),
						dependency("depends_b_c", "assignment_b", "assignment_c"),
					],
				),
			);
			await wait_for_graph(
				runtime,
				(graph) =>
					graph.assignments.filter(({ state }) => state === "running").length === 2,
			);

			expect(controlled.runs).toHaveLength(2);
			await Effect.runPromise(
				controlled.finish(controlled.find_run("assignment_a")!, "completed"),
			);
			const half_complete = await wait_for_graph(
				runtime,
				(graph) =>
					graph.assignments.find(({ assignment_id }) => assignment_id === "assignment_a")
						?.state === "complete",
			);

			expect(controlled.runs).toHaveLength(2);
			expect(
				half_complete.assignments.find(
					({ assignment_id }) => assignment_id === "assignment_c",
				),
			).toMatchObject({ state: "blocked" });

			await Effect.runPromise(
				controlled.finish(controlled.find_run("assignment_b")!, "completed"),
			);
			await wait_for_graph(
				runtime,
				(graph) =>
					graph.assignments.find(({ assignment_id }) => assignment_id === "assignment_c")
						?.state === "running",
			);

			expect(controlled.runs).toHaveLength(3);
		} finally {
			await runtime.dispose();
		}
	});

	it("propagates an unsuccessful predecessor through blocked dependents", async () => {
		const controlled = make_dependency_engine();
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [controlled.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			await route(
				runtime,
				start_command(
					"start_failure_chain",
					["assignment_a", "assignment_b", "assignment_c"],
					[
						dependency("depends_a_b", "assignment_a", "assignment_b"),
						dependency("depends_b_c", "assignment_b", "assignment_c"),
					],
				),
			);
			await wait_for_graph(
				runtime,
				(graph) =>
					graph.assignments.find(({ assignment_id }) => assignment_id === "assignment_a")
						?.state === "running",
			);

			await Effect.runPromise(
				controlled.finish(controlled.find_run("assignment_a")!, "failed"),
			);
			const failed = await wait_for_graph(runtime, (graph) =>
				graph.assignments.every(({ state }) => state === "failed"),
			);

			expect(controlled.runs).toHaveLength(1);
			expect(failed.group.state).toBe("failed");
			expect(failed.agent_runs).toHaveLength(3);
			expect(failed.agent_runs.every(({ state }) => state === "failed")).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects self, nonsensical, and cyclic dependencies without opening a run", async () => {
		const controlled = make_dependency_engine();
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [controlled.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			const self_dependency = await route(
				runtime,
				start_command(
					"start_self_dependency",
					["assignment_a", "assignment_b"],
					[dependency("depends_a_a", "assignment_a", "assignment_a")],
				),
			);
			const nonsensical_dependency = await route(
				runtime,
				start_command(
					"start_nonsensical_dependency",
					["assignment_a", "assignment_b"],
					[dependency("depends_group_b", "group_dependencies", "assignment_b")],
				),
			);
			const cycle = await route(
				runtime,
				start_command(
					"start_cycle",
					["assignment_a", "assignment_b"],
					[
						dependency("depends_a_b", "assignment_a", "assignment_b"),
						dependency("depends_b_a", "assignment_b", "assignment_a"),
					],
				),
			);

			expect(self_dependency).toMatchObject([
				{ payload: { error: { code: "orchestration.graph_invalid" }, status: "rejected" } },
			]);
			expect(nonsensical_dependency).toMatchObject([
				{ payload: { error: { code: "orchestration.graph_invalid" }, status: "rejected" } },
			]);
			expect(cycle).toMatchObject([
				{ payload: { error: { code: "orchestration.graph_invalid" }, status: "rejected" } },
			]);
			expect(controlled.runs).toHaveLength(0);
		} finally {
			await runtime.dispose();
		}
	});
});

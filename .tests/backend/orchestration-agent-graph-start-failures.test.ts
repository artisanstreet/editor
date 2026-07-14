import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Cause, Deferred, Effect, Queue, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	Engine,
	EngineObservation,
	EngineOpenInput,
	EngineRun,
	EngineRunTerminalState,
} from "@artisan/engines";
import type { CommandEnvelope, OrchestrationGraph, OutboundEnvelope } from "@artisan/protocol";
import { AgentGraphOrchestrator, make_backend_runtime, ProtocolRouter } from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

type StartBehavior = "fail_once" | "invalid_native" | "live";

function make_start_engine(id: string, behavior: StartBehavior) {
	let opens = 0;
	let scopes_closed = 0;
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

			opens += 1;
			yield* Effect.addFinalizer(() =>
				Effect.gen(function* () {
					scopes_closed += 1;
					yield* Queue.end(queue);
					yield* Deferred.succeed(closed, "closed");
				}),
			);

			if (behavior === "fail_once" && opens === 1) {
				return yield* Effect.die("The fake engine failed after allocating its scope");
			}

			const native_thread_id =
				behavior === "invalid_native" ? "" : `native:${input.artisan_run_id}`;

			return {
				artisan_run_id: input.artisan_run_id,
				Closed: Deferred.await(closed),
				Events: Stream.fromQueue(queue),
				native_thread_id,
				resume_token: { native_thread_id },
				Send: () => Effect.void,
			} satisfies EngineRun;
		});

	return {
		engine: {
			Descriptor: {
				capabilities,
				display_name: `Start boundary engine ${id}`,
				id,
				transport: "test",
			},
			Open,
			Probe: () => Effect.die("Probe is not used by graph start tests"),
		} satisfies Engine,
		opens: () => opens,
		scopes_closed: () => scopes_closed,
	};
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-agent-graph-start-"));

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
		thread_id: "thread_graph_start",
	};
}

function assignment(assignment_id: string, engine_id: string, max_attempts: number) {
	return {
		assignment_id,
		engine_id,
		expected_result: `${assignment_id} result`,
		instructions: `Work on ${assignment_id}`,
		max_attempts,
		parent_node_id: "group_graph_start",
		permission_policy: {
			approval: "never" as const,
			network_access: false,
			write_access: false,
		},
		profile: "default",
		role: "tester",
		scope: { kind: "test" as const, value: assignment_id, write_access: false },
		summary_contract: "Return a concise result",
		workspace: {
			isolation: "isolated" as const,
			workspace_id: `workspace_${assignment_id}`,
			working_directory: tmpdir(),
		},
	};
}

function start_command(first_engine_id: string, first_max_attempts: number) {
	return command("start_graph_boundaries", {
		assignments: [
			assignment("assignment_a", first_engine_id, first_max_attempts),
			assignment("assignment_b", "steady", 1),
		],
		group_id: "group_graph_start",
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

			return yield* graph.GetGraph("group_graph_start");
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

	throw new Error("Graph start boundary did not reach the expected state");
}

function accepted_sequence(output: ReadonlyArray<OutboundEnvelope>) {
	const receipt = output.find(({ kind }) => kind === "command.receipt");

	if (receipt?.kind !== "command.receipt" || receipt.payload.status === "rejected") {
		throw new Error("Expected an accepted graph command receipt");
	}

	return receipt.payload.journal_sequence;
}

async function create_thread(runtime: ReturnType<typeof make_backend_runtime>) {
	await route(
		runtime,
		command("create_graph_start_thread", {
			title: "Graph start boundaries",
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

describe("multi-agent graph start boundaries", () => {
	it("cleans a failed Engine scope and immediately dispatches the queued retry", async () => {
		const database_path = await make_database_path();
		const flaky = make_start_engine("flaky", "fail_once");
		const steady = make_start_engine("steady", "live");
		const runtime = make_backend_runtime({
			database_path,
			engines: [flaky.engine, steady.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			const started = await route(runtime, start_command("flaky", 2));
			const initial_sequence = accepted_sequence(started);
			const graph = await wait_for_graph(
				runtime,
				(current) =>
					current.assignments.every(({ state }) => state === "running") &&
					current.assignments.find(
						({ assignment_id }) => assignment_id === "assignment_a",
					)?.current_attempt === 2,
			);
			const flaky_runs = graph.agent_runs.filter(
				({ assignment_id }) => assignment_id === "assignment_a",
			);

			expect(flaky.opens()).toBe(2);
			expect(flaky.scopes_closed()).toBe(1);
			expect(flaky_runs.map(({ state }) => state)).toEqual(["failed", "running"]);
			expect(graph.journal_sequence).toBeGreaterThan(initial_sequence);
		} finally {
			await runtime.dispose();
		}

		expect(flaky.scopes_closed()).toBe(2);
		expect(steady.scopes_closed()).toBe(1);
	});

	it("rolls back activation and closes the spawned scope when projection validation fails", async () => {
		const database_path = await make_database_path();
		const invalid = make_start_engine("invalid", "invalid_native");
		const steady = make_start_engine("steady", "live");
		const runtime = make_backend_runtime({
			database_path,
			engines: [invalid.engine, steady.engine],
			migrations_path,
		});

		try {
			await create_thread(runtime);
			const started = await route(runtime, start_command("invalid", 1));
			const initial_sequence = accepted_sequence(started);
			const graph = await wait_for_graph(
				runtime,
				(current) =>
					current.assignments.find(
						({ assignment_id }) => assignment_id === "assignment_a",
					)?.state === "failed" &&
					current.assignments.find(
						({ assignment_id }) => assignment_id === "assignment_b",
					)?.state === "running",
			);
			const failed_run = graph.agent_runs.find(
				({ assignment_id }) => assignment_id === "assignment_a",
			)!;

			expect(invalid.opens()).toBe(1);
			expect(invalid.scopes_closed()).toBe(1);
			expect(failed_run).toMatchObject({ state: "failed" });
			expect(failed_run.native_thread_id).toBeUndefined();
			expect(failed_run.raw_origin).toBeUndefined();
			expect(graph.journal_sequence).toBeGreaterThan(initial_sequence);
		} finally {
			await runtime.dispose();
		}

		expect(steady.scopes_closed()).toBe(1);
	});
});

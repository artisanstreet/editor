import { Effect, Stream } from "effect";

import type { CommandEnvelope } from "@artisan/protocol";
import type { Engine, EngineRun } from "@artisan/engines";
import { AgentGraphOrchestrator, make_backend_runtime, ProtocolRouter } from "@artisan/backend";

const database_path = process.env.ARTISAN_GRAPH_CRASH_DATABASE;
const migrations_path = process.env.ARTISAN_GRAPH_CRASH_MIGRATIONS;

if (!database_path || !migrations_path) {
	throw new Error("Graph crash fixture paths are required");
}

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
const engine: Engine = {
	Descriptor: {
		capabilities,
		display_name: "Crash fixture engine",
		id: "crash-engine",
		transport: "test",
	},
	Open: (input) =>
		Effect.gen(function* () {
			yield* Effect.addFinalizer(() => Effect.void);

			return {
				artisan_run_id: input.artisan_run_id,
				Closed: Effect.never,
				Events: Stream.never,
				native_thread_id: `native:${input.artisan_run_id}`,
				resume_token: { native_thread_id: `native:${input.artisan_run_id}` },
				Send: () => Effect.void,
			} satisfies EngineRun;
		}),
	Probe: () => Effect.die("Probe is not used by the crash fixture"),
};
const runtime = make_backend_runtime({ database_path, engines: [engine], migrations_path });

function command(message_id: string, payload: CommandEnvelope["payload"]): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		thread_id: "thread_crash_graph",
	};
}

const assignment = (assignment_id: string) => ({
	assignment_id,
	engine_id: "crash-engine",
	expected_result: "Recovered result",
	instructions: `Work on ${assignment_id}`,
	max_attempts: 2,
	parent_node_id: "group_crash_graph",
	permission_policy: { approval: "never" as const, network_access: false, write_access: false },
	profile: "default",
	role: "tester",
	scope: { kind: "test" as const, value: assignment_id, write_access: false },
	summary_contract: "Return a concise result",
	workspace: {
		isolation: "isolated" as const,
		workspace_id: `workspace_${assignment_id}`,
		working_directory: process.cwd(),
	},
});

const Route = (input: CommandEnvelope) =>
	runtime.runPromise(
		Effect.gen(function* () {
			const router = yield* ProtocolRouter;

			return yield* router.Route(input);
		}),
	);

await Route(command("create_crash_graph_thread", { title: "Crash graph", type: "thread.create" }));
await Route(
	command("start_crash_graph", {
		assignments: [assignment("assignment_a"), assignment("assignment_b")],
		group_id: "group_crash_graph",
		type: "orchestration.group.start",
	}),
);

let ready = false;

for (let attempt = 0; attempt < 200; attempt += 1) {
	const is_ready = await runtime.runPromise(
		Effect.gen(function* () {
			const graph = yield* AgentGraphOrchestrator;
			const projection = yield* graph.GetGraph("group_crash_graph");

			return projection.agent_runs.every(({ state }) => state === "running");
		}),
	);

	if (is_ready) {
		ready = true;
		process.stdout.write("GRAPH_CRASH_READY\n");
		break;
	}

	await new Promise<void>((resolve) => setTimeout(resolve, 10));
}

if (!ready) {
	throw new Error("Graph crash fixture did not activate its runs");
}

await new Promise<never>(() => {});

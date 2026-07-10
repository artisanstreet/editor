import { spawn } from "node:child_process";
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
import type { OrchestrationGraph } from "@artisan/protocol";
import { AgentGraphOrchestrator, make_backend_runtime } from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const crash_fixture_path = fileURLToPath(
	new URL("./orchestration-agent-graph-crash-fixture.ts", import.meta.url),
);
const typescript_loader_url = new URL("./orchestration-agent-graph-loader.mjs", import.meta.url)
	.href;
const temporary_directories: Array<string> = [];

function make_recovery_engine() {
	const opened: Array<EngineOpenInput> = [];
	let scopes_closed = 0;
	const capability_names = [
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
	] as const;
	const capabilities = Object.fromEntries(
		capability_names.map((name) => [name, { state: "supported" as const }]),
	) as Engine["Descriptor"]["capabilities"];
	const Open = (input: EngineOpenInput) =>
		Effect.gen(function* () {
			const queue = yield* Queue.unbounded<EngineObservation, Cause.Done<void>>();
			const closed = yield* Deferred.make<EngineRunTerminalState>();

			opened.push(input);
			yield* Effect.addFinalizer(() =>
				Effect.gen(function* () {
					scopes_closed += 1;
					yield* Queue.end(queue);
					yield* Deferred.succeed(closed, "closed");
				}),
			);

			return {
				artisan_run_id: input.artisan_run_id,
				Closed: Deferred.await(closed),
				Events: Stream.fromQueue(queue),
				native_thread_id: `recovered:${input.artisan_run_id}`,
				resume_token: { native_thread_id: `recovered:${input.artisan_run_id}` },
				Send: () => Effect.void,
			} satisfies EngineRun;
		});

	return {
		engine: {
			Descriptor: {
				capabilities,
				display_name: "Recovery graph engine",
				id: "crash-engine",
				transport: "test",
			},
			Open,
			Probe: () => Effect.die("Probe is not used by graph recovery tests"),
		} satisfies Engine,
		opened,
		scopes_closed: () => scopes_closed,
	};
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-agent-graph-recovery-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

async function leave_crashed_graph(database_path: string) {
	const child = spawn(
		process.execPath,
		["--experimental-loader", typescript_loader_url, crash_fixture_path],
		{
			cwd: process.cwd(),
			env: {
				...process.env,
				ARTISAN_GRAPH_CRASH_DATABASE: database_path,
				ARTISAN_GRAPH_CRASH_MIGRATIONS: migrations_path,
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
				() => reject(new Error(`Graph crash fixture did not become ready: ${stderr}`)),
				15_000,
			);

			child.stdout.on("data", (chunk) => {
				stdout += String(chunk);

				if (stdout.includes("GRAPH_CRASH_READY")) {
					clearTimeout(timeout);
					resolve();
				}
			});
			child.once("exit", (code) => {
				clearTimeout(timeout);
				reject(new Error(`Graph crash fixture exited early with ${code}: ${stderr}`));
			});
		});
	} finally {
		if (child.exitCode === null && child.signalCode === null) {
			child.kill();
		}

		if (child.exitCode === null && child.signalCode === null) {
			await new Promise<void>((resolve, reject) => {
				const timeout = setTimeout(
					() => reject(new Error("Graph crash fixture did not terminate")),
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

function get_graph(runtime: ReturnType<typeof make_backend_runtime>) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const graph = yield* AgentGraphOrchestrator;

			return yield* graph.GetGraph("group_crash_graph");
		}),
	);
}

async function wait_for_graph(
	runtime: ReturnType<typeof make_backend_runtime>,
	predicate: (graph: OrchestrationGraph) => boolean,
) {
	for (let attempt = 0; attempt < 400; attempt += 1) {
		const graph = await get_graph(runtime);

		if (predicate(graph)) {
			return graph;
		}

		await new Promise<void>((resolve) => setTimeout(resolve, 10));
	}

	throw new Error("Recovered graph did not reach the expected state");
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("multi-agent graph restart recovery", () => {
	it("terminalizes stale owner attempts before dispatching monotonic retries", async () => {
		const database_path = await make_database_path();

		await leave_crashed_graph(database_path);

		const recovery = make_recovery_engine();
		const runtime = make_backend_runtime({
			database_path,
			engines: [recovery.engine],
			migrations_path,
		});

		try {
			const graph = await wait_for_graph(
				runtime,
				(current) =>
					current.assignments.every(
						({ current_attempt, state }) =>
							current_attempt === 2 && state === "running",
					) &&
					current.agent_runs.filter(
						({ attempt, state }) => attempt === 2 && state === "running",
					).length === 2,
			);
			const first_attempts = graph.agent_runs.filter(({ attempt }) => attempt === 1);
			const second_attempts = graph.agent_runs.filter(({ attempt }) => attempt === 2);

			expect(first_attempts).toHaveLength(2);
			expect(first_attempts.every(({ state }) => state === "failed")).toBe(true);
			expect(second_attempts).toHaveLength(2);
			expect(second_attempts.every(({ state }) => state === "running")).toBe(true);
			expect(recovery.opened).toHaveLength(2);
			expect(new Set(recovery.opened.map(({ artisan_run_id }) => artisan_run_id))).toEqual(
				new Set(second_attempts.map(({ run_id }) => run_id)),
			);
		} finally {
			await runtime.dispose();
		}

		expect(recovery.scopes_closed()).toBe(2);
	}, 45_000);
});

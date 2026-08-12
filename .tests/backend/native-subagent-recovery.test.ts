import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { EngineSubagentObservation } from "@artisan/engines";
import { AgentGraphRepository, make_backend_runtime } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration/repository";
import {
	AgentRuns,
	Assignments,
	NativeSubagentBindings,
	OrchestrationRuns,
} from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const timestamp = "2026-08-12T12:00:00.000Z";

const MakeDatabasePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-native-recovery-"));
	temporary_directories.push(directory);
	return join(directory, "artisan.db");
};

const InsertRoot = Effect.gen(function* () {
	const database = yield* Database;
	yield* database.client.insert(OrchestrationRuns).values({
		agent_id: "root-agent",
		created_at: timestamp,
		engine_id: "codex",
		native_resume_json: null,
		native_thread_id: "native-root",
		run_id: "root-run",
		status: "running",
		thread_id: "thread-native",
		updated_at: timestamp,
		working_directory: "C:\\workspace",
	});
});

const NativeChild: EngineSubagentObservation = {
	_tag: "subagent",
	agent_native_thread_id: "native-child",
	artisan_run_id: "root-run",
	activity: "Investigating crash recovery",
	observation_id: "native-child-running",
	parent_native_thread_id: "native-root",
	raw: {
		engine_id: "codex",
		frame: {},
		native_id: "native-child-running",
		transport: "test",
	},
	sequence: 1,
	state: "running",
	turn_id: "native-child-turn",
};

const ReadChildState = Effect.gen(function* () {
	const database = yield* Database;
	const [binding] = yield* database.client.select().from(NativeSubagentBindings);
	const [assignment] = yield* database.client.select().from(Assignments);
	const [run] = yield* database.client.select().from(AgentRuns);
	return { assignment, binding, run };
});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("provider-native subagent crash recovery", () => {
	it("replays a committed child observation before settling its stale root", async () => {
		const runtime = make_backend_runtime({
			database_path: await MakeDatabasePath(),
			engines: [],
			migrations_path,
		});
		try {
			await runtime.runPromise(InsertRoot);
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					const graph = yield* AgentGraphRepository;
					/** Simulates Forge dying after inbox persistence but before graph projection. */
					yield* repository.RecordObservation(NativeChild);
					yield* graph.RecoverObservedSubagents;
					yield* repository.ClaimNativeRecoveries();
					yield* graph.ReconcileObservedSubagentsExcept(new Set<string>());
				}),
			);

			const settled = await runtime.runPromise(ReadChildState);
			expect(settled.binding).toMatchObject({ state: "stopped" });
			expect(settled.assignment).toMatchObject({ state: "stopped" });
			expect(settled.run).toMatchObject({ state: "stopped" });
		} finally {
			await runtime.dispose();
		}
	});

	it("settles stale native children after their non-resumable root is claimed", async () => {
		const runtime = make_backend_runtime({
			database_path: await MakeDatabasePath(),
			engines: [],
			migrations_path,
		});
		try {
			await runtime.runPromise(InsertRoot);
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					const graph = yield* AgentGraphRepository;
					yield* repository.RecordObservation(NativeChild);
					yield* graph.RecordObservedSubagent(NativeChild);
					yield* repository.ClaimNativeRecoveries();
					yield* graph.ReconcileObservedSubagentsExcept(new Set<string>());
				}),
			);

			const settled = await runtime.runPromise(ReadChildState);
			expect(settled.binding).toMatchObject({ state: "stopped" });
			expect(settled.assignment).toMatchObject({ state: "stopped" });
			expect(settled.run).toMatchObject({ state: "stopped" });
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps children active only while their root is provisionally resuming", async () => {
		const runtime = make_backend_runtime({
			database_path: await MakeDatabasePath(),
			engines: [],
			migrations_path,
		});
		try {
			await runtime.runPromise(InsertRoot);
			await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					const graph = yield* AgentGraphRepository;
					yield* repository.RecordObservation(NativeChild);
					yield* graph.RecordObservedSubagent(NativeChild);
					yield* repository.ClaimNativeRecoveries();
					yield* graph.ReconcileObservedSubagentsExcept(new Set(["root-run"]));
				}),
			);

			expect(await runtime.runPromise(ReadChildState)).toMatchObject({
				assignment: { state: "running" },
				binding: { state: "running" },
				run: { state: "running" },
			});
			await runtime.runPromise(
				Effect.gen(function* () {
					const graph = yield* AgentGraphRepository;
					yield* graph.ReconcileObservedRoot("root-run");
				}),
			);
			expect(await runtime.runPromise(ReadChildState)).toMatchObject({
				assignment: { state: "stopped" },
				binding: { state: "stopped" },
				run: { state: "stopped" },
			});
		} finally {
			await runtime.dispose();
		}
	});
});

import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import {
	ReadRootThreadLiveStatus,
	ReconcileRootThreadLiveStatuses,
} from "../../modules/backend/src/persistence/orchestration/thread-lifecycle-status";
import {
	OrchestrationCoordinators,
	OrchestrationInteractions,
	OrchestrationRuns,
	Threads,
} from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-awaiting-answer-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

const now = "2026-08-20T10:00:00.000Z";

interface ScenarioInteraction {
	readonly kind: string;
	readonly state: string;
}

/**
 * One thread per scenario, inserts only: the states under test are what the
 * projection reads, not how the rows got there, and pure inserts keep this
 * test free of query-builder imports the suite root does not carry.
 */
const SetupScenario = (
	name: string,
	run_status: string,
	interactions: ReadonlyArray<ScenarioInteraction>,
) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at: now,
			thread_id: `thread_${name}`,
			title: name,
			updated_at: now,
		});
		yield* database.client.insert(OrchestrationCoordinators).values({
			active_run_id: `run_${name}`,
			agent_id: `agent_${name}`,
			created_at: now,
			display_name: "Agent",
			engine_id: "engine_1",
			role: "coordinator",
			thread_id: `thread_${name}`,
			updated_at: now,
		});
		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: `agent_${name}`,
			created_at: now,
			engine_id: "engine_1",
			run_id: `run_${name}`,
			status: run_status,
			thread_id: `thread_${name}`,
			updated_at: now,
			working_directory: "C:/work",
		});
		if (interactions.length > 0) {
			yield* database.client.insert(OrchestrationInteractions).values(
				interactions.map((interaction, index) => ({
					created_at: now,
					description: `${interaction.kind}_${index}`,
					interaction_id: `${interaction.kind}_${index}`,
					kind: interaction.kind,
					run_id: `run_${name}`,
					state: interaction.state,
					updated_at: now,
				})),
			);
		}
	});

const ReadLiveStatus = (name: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		return yield* ReadRootThreadLiveStatus(database.client, `thread_${name}`);
	});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread awaiting-answer status", () => {
	it("says Waiting for answer exactly while a live root run holds a requested question", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			const statuses = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SetupScenario("working", "running", []);
					yield* SetupScenario("waiting", "running", [
						{ kind: "question", state: "requested" },
					]);
					yield* SetupScenario("answered", "running", [
						{ kind: "question", state: "resolved" },
					]);
					/** An approval also waits on the reader, but it has its own card. */
					yield* SetupScenario("approving", "running", [
						{ kind: "approval", state: "requested" },
					]);
					/**
					 * The run died before the answer: a stale ask must not hold a
					 * settled thread in the waiting state.
					 */
					yield* SetupScenario("settled", "completed", [
						{ kind: "question", state: "requested" },
					]);

					return {
						answered: yield* ReadLiveStatus("answered"),
						approving: yield* ReadLiveStatus("approving"),
						settled: yield* ReadLiveStatus("settled"),
						waiting: yield* ReadLiveStatus("waiting"),
						working: yield* ReadLiveStatus("working"),
					};
				}),
			);

			expect(statuses.working).toBe("Working");
			expect(statuses.waiting).toBe("Waiting for answer");
			expect(statuses.answered).toBe("Working");
			expect(statuses.approving).toBe("Working");
			expect(statuses.settled).toBe("Complete");
		} finally {
			await runtime.dispose();
		}
	});

	it("recovers the waiting state through the bulk reconcile", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
		});

		try {
			const projected = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* SetupScenario("waiting", "running", [
						{ kind: "question", state: "requested" },
					]);
					yield* SetupScenario("answered", "running", [
						{ kind: "question", state: "resolved" },
					]);
					yield* ReconcileRootThreadLiveStatuses(database.client, now);

					const threads = yield* database.client
						.select({ live_status: Threads.live_status, thread_id: Threads.thread_id })
						.from(Threads);

					return new Map(threads.map((thread) => [thread.thread_id, thread.live_status]));
				}),
			);

			expect(projected.get("thread_waiting")).toBe("Waiting for answer");
			expect(projected.get("thread_answered")).toBe("Working");
		} finally {
			await runtime.dispose();
		}
	});
});

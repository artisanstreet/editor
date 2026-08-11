import { cp, mkdtemp, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	ConversationPatches,
	ConversationSources,
	NativeSubagentObservationInbox,
	NativeSubagentTranscriptInbox,
	OrchestrationRawObservations,
	OrchestrationRuns,
	SurfaceItems,
} from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const storage_migrations = new Set([
	"20260811035058_striped_chimera",
	"20260811040527_broken_la_nuit",
	"20260811040651_graceful_warlock",
]);
const directories: string[] = [];

const MakeDatabaseRuntime = (database_path: string, migrations: string) =>
	ManagedRuntime.make(
		Layer.mergeAll(make_database_layer({ database_path, migrations_path: migrations })),
	);

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("storage compaction migration", () => {
	it("retains canonical state while dropping historic transport payloads", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-storage-compaction-"));
		directories.push(directory);
		const database_path = join(directory, "artisan.db");
		const legacy_migrations = join(directory, "migrations");
		for (const entry of await readdir(migrations_path, { withFileTypes: true })) {
			if (!entry.isDirectory() || storage_migrations.has(entry.name)) continue;
			await cp(join(migrations_path, entry.name), join(legacy_migrations, entry.name), {
				recursive: true,
			});
		}

		const legacy = MakeDatabaseRuntime(database_path, legacy_migrations);
		try {
			await legacy.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.run(`
						INSERT INTO orchestration_runs
						(run_id, thread_id, agent_id, engine_id, model_id, working_directory, status,
						 native_thread_id, native_resume_json, created_at, updated_at)
						VALUES ('run_1', 'thread_1', 'agent_1', 'codex', NULL, '.', 'completed',
						 NULL, NULL, '2026-08-11T00:00:00.000Z', '2026-08-11T00:00:00.000Z')
					`);
					for (let sequence = 1; sequence <= 300; sequence += 1) {
						yield* database.client.run(`
							INSERT INTO orchestration_raw_observations
							(observation_id, run_id, engine_id, sequence, transport, frame_json)
							VALUES ('observation_${sequence}', 'run_1', 'codex', ${sequence}, 'test', '{"large":"payload"}')
						`);
						yield* database.client.run(`
							INSERT INTO conversation_sources
						(source_id, thread_id, journal_sequence, observed_at)
							VALUES ('observation:observation_${sequence}', 'thread_1', NULL, '2026-08-11T00:00:00.000Z')
						`);
						yield* database.client.run(`
							INSERT INTO conversation_patches (patch_id, thread_id, sequence, patch_json)
							VALUES ('patch_${sequence}', 'thread_1', ${sequence}, '{}')
						`);
					}
					for (let sequence = 1; sequence <= 600; sequence += 1) {
						yield* database.client.run(`
							INSERT INTO surface_items
							(surface_id, observation_id, thread_id, run_id, sequence, category, kind,
							 summary_json, occurred_at)
							VALUES ('surface_${sequence}', 'surface_observation_${sequence}', 'thread_1',
							 'run_1', ${sequence}, 'work', 'message', '{}', '2026-08-11T00:00:00.000Z')
						`);
					}
					yield* database.client.run(`
						INSERT INTO conversation_sources
						(source_id, thread_id, journal_sequence, observed_at)
						VALUES ('event:event_1', 'thread_1', 1, '2026-08-11T00:00:00.000Z')
					`);
					for (const table of [
						"native_subagent_transcript_inbox",
						"native_subagent_observation_inbox",
					]) {
						const transcript_columns =
							table === "native_subagent_transcript_inbox"
								? ", content_json"
								: ", state";
						const transcript_values =
							table === "native_subagent_transcript_inbox" ? ", '[]'" : ", 'running'";
						yield* database.client.run(`
							INSERT INTO ${table}
							(observation_id, root_run_id, engine_id, agent_native_thread_id,
							 parent_native_thread_id, sequence, created_at, processed_at${transcript_columns})
							VALUES ('${table}_processed', 'run_1', 'codex', 'child', 'root', 1,
							 '2026-08-11T00:00:00.000Z', '2026-08-11T00:00:01.000Z'${transcript_values})
						`);
					}
				}),
			);
		} finally {
			await legacy.dispose();
		}

		const compacted = MakeDatabaseRuntime(database_path, migrations_path);
		try {
			const state = await compacted.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						inbox: yield* database.client.select().from(NativeSubagentTranscriptInbox),
						observation_inbox: yield* database.client
							.select()
							.from(NativeSubagentObservationInbox),
						patches: yield* database.client.select().from(ConversationPatches),
						raw: yield* database.client.select().from(OrchestrationRawObservations),
						runs: yield* database.client.select().from(OrchestrationRuns),
						sources: yield* database.client.select().from(ConversationSources),
						surfaces: yield* database.client.select().from(SurfaceItems),
					};
				}),
			);

			expect(state.raw).toEqual([]);
			expect(state.sources.map((row) => row.source_id)).toEqual(["event:event_1"]);
			expect(state.patches).toHaveLength(256);
			expect(Math.min(...state.patches.map((row) => row.sequence))).toBe(45);
			expect(state.inbox).toEqual([]);
			expect(state.observation_inbox).toEqual([]);
			expect(state.runs[0]?.last_observation_sequence).toBe(300);
			expect(state.surfaces).toHaveLength(512);
			expect(Math.min(...state.surfaces.map((row) => row.sequence))).toBe(89);
		} finally {
			await compacted.dispose();
		}
	});
});

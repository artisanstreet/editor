import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime, ThreadErasure } from "@artisan/backend";
import type { EngineObservation } from "@artisan/engines";

import {
	ApplyEngineObservation,
	conversation_patch_replay_batch_size,
	ConversationReadModel,
} from "../../modules/backend/src/conversation";
import { Database } from "../../modules/backend/src/persistence/database";
import {
	ConversationItems,
	ConversationPatches,
	ConversationSources,
	ConversationThreads,
	ConversationTurns,
	EventStreams,
	Threads,
} from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-forge-persistence-bounds-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

const Delta = (sequence: number): EngineObservation =>
	({
		_tag: "agent_message_delta",
		artisan_run_id: "run_1",
		delta: "x",
		item_id: "assistant_1",
		observation_id: `observation_${sequence}`,
		phase: "unspecified",
		raw: { engine_id: "codex", frame: {}, transport: "test" },
		sequence,
		turn_id: "turn_1",
	}) as EngineObservation;

describe("Forge persistence bounds", () => {
	it("bounds patch replay, avoids repeated unchanged turn patches, and erases every conversation row", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const read_model = yield* ConversationReadModel;
					const erasure = yield* ThreadErasure;
					const now = "2026-07-27T00:00:00.000Z";
					yield* database.client.insert(Threads).values({
						created_at: now,
						last_activity_at: now,
						thread_id: "thread_1",
						title: "Conversation",
						updated_at: now,
					});
					yield* database.client.insert(EventStreams).values({
						last_sequence: 0,
						stream_id: "thread:thread_1",
					});
					yield* database.client.transaction((transaction) =>
						Effect.forEach(
							Array.from(
								{ length: conversation_patch_replay_batch_size + 1 },
								(_, index) => Delta(index + 1),
							),
							(observation) =>
								ApplyEngineObservation(transaction, observation, {
									occurred_at: now,
									run_id: "run_1",
									thread_id: "thread_1",
								}) as Effect.Effect<unknown, unknown, never>,
							{ discard: true },
						),
					);
					const replay = yield* read_model.ReadPatches("thread_1", 0);
					const persisted = yield* database.client.select().from(ConversationPatches);
					const erased = yield* erasure.CleanupExpired(now, "2026-07-28T00:00:00.000Z");
					const remaining = yield* Effect.all([
						database.client.select().from(ConversationItems),
						database.client.select().from(ConversationPatches),
						database.client.select().from(ConversationSources),
						database.client.select().from(ConversationThreads),
						database.client.select().from(ConversationTurns),
					]);
					return { erased, persisted, remaining, replay };
				}),
			);

			expect(result.replay).toHaveLength(conversation_patch_replay_batch_size);
			expect(
				result.persisted.filter((row) => JSON.parse(row.patch_json).type === "turn_upsert"),
			).toHaveLength(1);
			expect(result.erased).toEqual(["thread_1"]);
			expect(result.remaining).toEqual([[], [], [], [], []]);
		} finally {
			await runtime.dispose();
		}
	});
});

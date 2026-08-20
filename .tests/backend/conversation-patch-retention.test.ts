import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";
import type { EngineObservation } from "@artisan/engines";

import {
	ApplyEngineObservation,
	conversation_patch_retention_limit,
} from "../../modules/backend/src/conversation";
import { Database } from "../../modules/backend/src/persistence/database";
import {
	ConversationPatches,
	EventStreams,
	Threads,
} from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-patch-retention-"));
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

describe("conversation patch retention", () => {
	it("keeps only a bounded live-delivery window", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const patches = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const now = "2026-08-11T00:00:00.000Z";
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
								{ length: conversation_patch_retention_limit + 384 },
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
					return yield* database.client.select().from(ConversationPatches);
				}),
			);

			expect(patches.length).toBeLessThanOrEqual(conversation_patch_retention_limit);
			expect(patches[0]?.sequence).toBeGreaterThan(1);
			/** One turn_upsert patch precedes the appended deltas. */
			expect(patches.at(-1)?.sequence).toBe(conversation_patch_retention_limit + 385);
		} finally {
			await runtime.dispose();
		}
	});
});

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
	ConversationReadModel,
} from "../../modules/backend/src/conversation";
import { Database } from "../../modules/backend/src/persistence/database";
import { ConversationItems, Threads } from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-thinking-progress-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

const context = {
	occurred_at: "2026-08-17T06:18:00.000Z",
	run_id: "run_1",
	thread_id: "thread_1",
};

const raw = { engine_id: "claude", frame: {}, transport: "test" } as const;

const Progress = (sequence: number, thinking_tokens: number): EngineObservation => ({
	_tag: "reasoning_summary_delta",
	artisan_run_id: "run_1",
	delta: "",
	item_id: "message_1:reasoning",
	observation_id: `observation_progress_${sequence}`,
	raw,
	sequence,
	summary_index: 0,
	thinking_tokens,
	turn_id: "turn_1",
});

const Seed = Effect.gen(function* () {
	const database = yield* Database;
	yield* database.client.insert(Threads).values({
		created_at: "2026-08-17T06:00:00.000Z",
		last_activity_at: "2026-08-17T06:00:00.000Z",
		thread_id: "thread_1",
		title: "Conversation",
		updated_at: "2026-08-17T06:00:00.000Z",
	});
	return database;
});

describe("conversation thinking progress", () => {
	it("opens one streaming reasoning row for the count and replaces rather than appends it", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Seed;
					const read_model = yield* ConversationReadModel;
					yield* database.client.transaction((transaction) =>
						Effect.gen(function* () {
							for (const observation of [
								Progress(1, 150),
								Progress(2, 900),
								Progress(3, 900),
							]) {
								yield* ApplyEngineObservation(
									transaction,
									observation,
									context,
								) as Effect.Effect<unknown, unknown, never>;
							}
						}),
					);
					return {
						patches: yield* read_model.ReadPatches("thread_1", 0),
						snapshot: yield* read_model.ReadSnapshot("thread_1"),
					};
				}),
			);

			expect(result.snapshot.status).toBe("available");
			if (result.snapshot.status !== "available") return;
			const reasoning = result.snapshot.snapshot.items.filter(
				(item) => item.type === "reasoning_summary",
			);
			expect(reasoning).toHaveLength(1);
			expect(reasoning[0]).toMatchObject({
				id: "message_1:reasoning",
				lifecycle: "streaming",
				text: "",
				thinking_tokens: 900,
			});
			/** An unchanged count is not a revision: 150 → 900 is one upsert after the open. */
			const upserts = result.patches.filter(
				(patch) =>
					patch.type === "item_upsert" &&
					patch.item.type === "reasoning_summary" &&
					patch.item.id === "message_1:reasoning",
			);
			expect(
				upserts.map((patch) => (patch as { item: { revision: number } }).item.revision),
			).toEqual([0, 1]);
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps a settled phase's final count when a late estimate arrives", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Seed;
					const read_model = yield* ConversationReadModel;
					yield* database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* ApplyEngineObservation(
								transaction,
								Progress(1, 400),
								context,
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyEngineObservation(
								transaction,
								{
									_tag: "reasoning_summary_completed",
									artisan_run_id: "run_1",
									item_id: "message_1:reasoning",
									observation_id: "observation_completed",
									raw,
									sequence: 2,
									turn_id: "turn_1",
								},
								context,
							) as Effect.Effect<unknown, unknown, never>;
							yield* ApplyEngineObservation(
								transaction,
								Progress(3, 999),
								context,
							) as Effect.Effect<unknown, unknown, never>;
						}),
					);
					return yield* read_model.ReadSnapshot("thread_1");
				}),
			);

			expect(result.status).toBe("available");
			if (result.status !== "available") return;
			expect(
				result.snapshot.items.find((item) => item.id === "message_1:reasoning"),
			).toMatchObject({ lifecycle: "completed", thinking_tokens: 400 });
		} finally {
			await runtime.dispose();
		}
	});

	it("still reads durable rows written with a thinking-token count", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Seed;
					const read_model = yield* ConversationReadModel;
					yield* database.client.transaction((transaction) =>
						ApplyEngineObservation(transaction, Progress(1, 150), context),
					);
					/**
					 * The row exactly as production 0.2.74 persisted it. A reader that
					 * rejects it makes the whole thread unopenable, which is the outage
					 * this guards against.
					 */
					yield* database.client.update(ConversationItems).set({
						entity_json: JSON.stringify({
							agent_id: "agent_21011518715854848",
							created_at: "2026-08-17T06:18:00.699Z",
							id: "message_1:reasoning",
							lifecycle: "completed",
							ordinal: 4752,
							references: [],
							revision: 8,
							run_id: "run_1",
							source_refs: [{ provider: "engine", reference: "message_1:reasoning" }],
							updated_at: "2026-08-17T06:18:11.340Z",
							turn_id: "turn_1",
							text: "",
							thinking_tokens: 900,
							type: "reasoning_summary",
						}),
					});
					return yield* read_model.ReadSnapshot("thread_1");
				}),
			);

			expect(result.status).toBe("available");
			if (result.status !== "available") return;
			expect(
				result.snapshot.items.find((item) => item.id === "message_1:reasoning"),
			).toMatchObject({ thinking_tokens: 900, type: "reasoning_summary" });
		} finally {
			await runtime.dispose();
		}
	});
});

import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";

import { ConversationReadModel } from "../../modules/backend/src/conversation";
import { Database } from "../../modules/backend/src/persistence/database";
import {
	ConversationItems,
	ConversationThreads,
	ConversationTurns,
	Threads,
} from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-conversation-window-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

const created_at = "2026-08-22T12:00:00.000Z";

const turn_entity = (id: string, ordinal: number) => ({
	created_at,
	id,
	lifecycle: "completed",
	ordinal,
	references: [],
	revision: 0,
	source_refs: [],
	type: "turn",
	updated_at: created_at,
});

const item_entity = (
	id: string,
	ordinal: number,
	turn_id: string,
	type: "user_message" | "assistant_message",
	text: string,
) => ({
	created_at,
	id,
	lifecycle: "completed",
	ordinal,
	...(type === "assistant_message" ? { phase: "final" } : {}),
	references: [],
	revision: 0,
	source_refs: [],
	text,
	turn_id,
	type,
	updated_at: created_at,
});

/** Four settled exchanges: turn ordinals 0/3/6/9, each with a user and a reply. */
const SeedThread = Effect.gen(function* () {
	const database = yield* Database;
	yield* database.client.insert(Threads).values({
		created_at,
		last_activity_at: created_at,
		thread_id: "thread_1",
		title: "Windowed",
		updated_at: created_at,
	});
	yield* database.client.insert(ConversationThreads).values({
		journal_sequence: 0,
		last_patch_sequence: 12,
		next_ordinal: 12,
		thread_id: "thread_1",
		updated_at: created_at,
	});
	for (const index of [0, 1, 2, 3]) {
		const turn_id = `turn_${index}`;
		const base = index * 3;
		yield* database.client.insert(ConversationTurns).values({
			entity_json: JSON.stringify(turn_entity(turn_id, base)),
			ordinal: base,
			thread_id: "thread_1",
			turn_id,
		});
		yield* database.client.insert(ConversationItems).values({
			entity_json: JSON.stringify(
				item_entity(`user_${index}`, base + 1, turn_id, "user_message", `ask ${index}`),
			),
			item_id: `user_${index}`,
			ordinal: base + 1,
			thread_id: "thread_1",
			turn_id,
		});
		yield* database.client.insert(ConversationItems).values({
			entity_json: JSON.stringify(
				item_entity(
					`reply_${index}`,
					base + 2,
					turn_id,
					"assistant_message",
					`answer ${index}`,
				),
			),
			item_id: `reply_${index}`,
			ordinal: base + 2,
			thread_id: "thread_1",
			turn_id,
		});
	}
});

describe("windowed conversation reads", () => {
	it("bounds a snapshot to the newest turns and carries whole-thread markers", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const availability = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const read_model = yield* ConversationReadModel;
					return yield* read_model.ReadSnapshot("thread_1", {
						window: { maximum_turn_count: 2 },
					});
				}),
			);

			expect(availability.status).toBe("available");
			if (availability.status !== "available") return;
			const snapshot = availability.snapshot;
			expect(snapshot.turns.map((turn) => turn.id)).toEqual(["turn_2", "turn_3"]);
			expect(snapshot.items.map((item) => item.id)).toEqual([
				"user_2",
				"reply_2",
				"user_3",
				"reply_3",
			]);
			expect(snapshot.last_patch_sequence).toBe(12);
			expect(snapshot.window).toMatchObject({ total_turn_count: 4 });
			expect(snapshot.window?.markers).toEqual([
				{ id: "user_0", label: "ask 0", ordinal: 1, turn_ordinal: 0 },
				{ id: "user_1", label: "ask 1", ordinal: 4, turn_ordinal: 3 },
				{ id: "user_2", label: "ask 2", ordinal: 7, turn_ordinal: 6 },
				{ id: "user_3", label: "ask 3", ordinal: 10, turn_ordinal: 9 },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("returns older ranges beneath a floor without window metadata", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const [range, bounded, exhausted] = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const read_model = yield* ConversationReadModel;
					return yield* Effect.all([
						read_model.ReadSnapshot("thread_1", {
							range: { before_turn_ordinal: 6, maximum_turn_count: 8 },
						}),
						read_model.ReadSnapshot("thread_1", {
							range: {
								before_turn_ordinal: 6,
								maximum_turn_count: 8,
								minimum_turn_ordinal: 3,
							},
						}),
						read_model.ReadSnapshot("thread_1", {
							range: { before_turn_ordinal: 0, maximum_turn_count: 8 },
						}),
					]);
				}),
			);

			expect(range.status).toBe("available");
			if (range.status !== "available") return;
			expect(range.snapshot.turns.map((turn) => turn.id)).toEqual(["turn_0", "turn_1"]);
			expect(range.snapshot.items.map((item) => item.id)).toEqual([
				"user_0",
				"reply_0",
				"user_1",
				"reply_1",
			]);
			expect(range.snapshot.window).toBeUndefined();

			expect(bounded.status).toBe("available");
			if (bounded.status !== "available") return;
			expect(bounded.snapshot.turns.map((turn) => turn.id)).toEqual(["turn_1"]);

			expect(exhausted.status).toBe("available");
			if (exhausted.status !== "available") return;
			expect(exhausted.snapshot.turns).toEqual([]);
			expect(exhausted.snapshot.items).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps a full read byte-identical to the unbounded shape", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const availability = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const read_model = yield* ConversationReadModel;
					return yield* read_model.ReadSnapshot("thread_1");
				}),
			);

			expect(availability.status).toBe("available");
			if (availability.status !== "available") return;
			expect(availability.snapshot.turns).toHaveLength(4);
			expect(availability.snapshot.items).toHaveLength(8);
			expect(availability.snapshot.window).toBeUndefined();
		} finally {
			await runtime.dispose();
		}
	});
});

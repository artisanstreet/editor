import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";
import type { ThreadListItem } from "@artisan/protocol";

import { ConversationReadModel } from "../../modules/backend/src/conversation";
import { Database } from "../../modules/backend/src/persistence/database";
import {
	ConversationThreads,
	ConversationTurns,
	Threads,
} from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-conversation-open-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

const ResolvedThread = (thread_id: string): ThreadListItem =>
	({
		activity_version: 1,
		affinity_version: 1,
		created_at: "2026-08-14T00:00:00.000Z",
		last_activity_at: "2026-08-14T00:00:00.000Z",
		linked_projects: [],
		live_status: "Idle",
		metadata_version: 1,
		pinned: false,
		project_affinity_scores: [],
		project_locked: false,
		thread_id,
		title: "Conversation",
		title_locked: false,
		title_source: "initial",
		updated_at: "2026-08-14T00:00:00.000Z",
	}) as ThreadListItem;

describe("conversation open snapshot", () => {
	it("uses the fast path only when a conversation head exists and preserves the full empty fallback", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const conversations = yield* ConversationReadModel;
					const thread = ResolvedThread("thread_open_empty");
					yield* database.client.insert(Threads).values({
						created_at: thread.created_at,
						last_activity_at: thread.last_activity_at,
						thread_id: thread.thread_id,
						title: thread.title,
						updated_at: thread.updated_at,
					});
					return {
						direct: yield* conversations.ReadSnapshot(thread.thread_id),
						open: yield* conversations.ReadOpenSnapshot(thread),
					};
				}),
			);

			expect(Option.isNone(result.open)).toBe(true);
			expect(result.direct).toMatchObject({
				status: "available",
				snapshot: { items: [], thread_id: "thread_open_empty", turns: [] },
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps malformed stored entities as typed projection failures", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			await expect(
				runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const conversations = yield* ConversationReadModel;
						const thread = ResolvedThread("thread_open_corrupt");
						yield* database.client.insert(ConversationThreads).values({
							journal_sequence: 1,
							last_patch_sequence: 0,
							next_ordinal: 1,
							thread_id: thread.thread_id,
							updated_at: thread.updated_at,
						});
						yield* database.client.insert(ConversationTurns).values({
							entity_json: "{",
							ordinal: 0,
							thread_id: thread.thread_id,
							turn_id: "turn_corrupt",
						});
						return yield* conversations.ReadOpenSnapshot(thread);
					}),
				),
			).rejects.toMatchObject({
				_tag: "ConversationReadModelFailure",
				cause: { _tag: "ConversationProjectionError" },
			});
		} finally {
			await runtime.dispose();
		}
	});
});

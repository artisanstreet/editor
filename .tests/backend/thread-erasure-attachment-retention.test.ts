import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime, ThreadErasure } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import {
	EventStreams,
	JournalCommands,
	MessageImageAttachments,
	Threads,
} from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const now = "2026-08-11T00:00:00.000Z";

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-thread-erasure-attachments-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread erasure attachment retention", () => {
	it("erases image bytes owned by the thread's accepted messages only", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					for (const thread_id of ["thread_erased", "thread_retained"]) {
						yield* database.client.insert(Threads).values({
							created_at: now,
							last_activity_at: now,
							pinned: thread_id === "thread_retained",
							thread_id,
							title: thread_id,
							updated_at: now,
						});
						yield* database.client.insert(EventStreams).values({
							last_sequence: 0,
							stream_id: `thread:${thread_id}`,
						});
						yield* database.client.insert(JournalCommands).values({
							accepted_at: now,
							agent_id: null,
							assigned_run_id: null,
							causation_id: null,
							message_id: `message_${thread_id}`,
							origin: "client",
							payload_json: '{"type":"thread.message"}',
							payload_type: "thread.message",
							raw_origin_json: null,
							run_id: null,
							schema_version: 1,
							sent_at: now,
							status: "accepted",
							thread_id,
						});
						yield* database.client.insert(MessageImageAttachments).values({
							attachment_id: `attachment_${thread_id}`,
							content: Buffer.from(thread_id),
							media_type: "image/webp",
							message_id: `message_${thread_id}`,
							name: `${thread_id}.webp`,
							position: 0,
							size_bytes: thread_id.length,
						});
					}

					const erased = yield* erasure.CleanupExpired(now, "2026-08-12T00:00:00.000Z");
					const attachments = yield* database.client
						.select({ message_id: MessageImageAttachments.message_id })
						.from(MessageImageAttachments);

					return { attachments, erased };
				}),
			);

			expect(result.erased).toContain("thread_erased");
			expect(result.attachments).toEqual([{ message_id: "message_thread_retained" }]);
		} finally {
			await runtime.dispose();
		}
	});
});

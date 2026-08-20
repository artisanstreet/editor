import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { DatabaseSync } from "node:sqlite";

import { afterEach, describe, expect, it } from "vitest";
import { Effect } from "effect";

import type { CommandEnvelope } from "@artisan/protocol";
import { ProtocolRouter, make_backend_runtime } from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-scratch-"));
	temporary_directories.push(directory);
	return join(directory, "artisan.db");
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) =>
				rm(directory, { force: true, recursive: true }).catch(() => undefined),
			),
	);
});

describe("scratch journal dump", () => {
	it("dumps all journal rows after one thread.create", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		const command: CommandEnvelope = {
			protocol_version: 1,
			schema_version: 1,
			kind: "command",
			message_id: "message_1",
			thread_id: "thread_1",
			origin: "frontend",
			sent_at: "2026-07-10T08:00:00.000Z",
			payload: { type: "thread.create", title: "Backend foundation" },
		};

		try {
			const output = await runtime.runPromise(
				Effect.gen(function* () {
					const router = yield* ProtocolRouter;
					return yield* router.Route(command);
				}),
			);
			console.log("ROUTE OUTPUT KINDS:", JSON.stringify(output.map((o: any) => o.kind)));

			const db = new DatabaseSync(database_path, { readOnly: true });
			const events = db
				.prepare(
					"SELECT sequence, stream_id, stream_sequence, event_type, origin FROM journal_events ORDER BY sequence",
				)
				.all();
			const commands = db
				.prepare("SELECT message_id, payload_type, status FROM journal_commands")
				.all();
			const streams = db.prepare("SELECT stream_id, last_sequence FROM event_streams").all();
			db.close();

			console.log("JOURNAL_EVENTS:", JSON.stringify(events, null, 1));
			console.log("JOURNAL_COMMANDS:", JSON.stringify(commands, null, 1));
			console.log("EVENT_STREAMS:", JSON.stringify(streams, null, 1));

			expect(events.length).toBeGreaterThan(0);
		} finally {
			await runtime.dispose();
		}
	});
});

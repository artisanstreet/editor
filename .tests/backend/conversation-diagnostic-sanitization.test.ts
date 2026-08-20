import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";

import {
	ApplyEngineObservation,
	ConversationReadModel,
} from "../../modules/backend/src/conversation";
import { Database } from "../../modules/backend/src/persistence/database";
import { Threads } from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-conversation-diagnostic-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("conversation diagnostic projection", () => {
	it("removes terminal controls from public process diagnostics while retaining private provenance", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const availability = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const read_model = yield* ConversationReadModel;
					yield* database.client.insert(Threads).values({
						created_at: "2026-08-12T00:00:00.000Z",
						last_activity_at: "2026-08-12T00:00:00.000Z",
						thread_id: "thread_1",
						title: "Conversation",
						updated_at: "2026-08-12T00:00:00.000Z",
					});
					yield* database.client.transaction((transaction) =>
						ApplyEngineObservation(
							transaction,
							{
								_tag: "process_diagnostic",
								artisan_run_id: "run_1",
								error_ref: {
									artisan_code: "AE-UNKNOWN-000",
									detail: "\u001b[31mNative\u001b[0m \u001b]8;;https://example.test\u0007failure\u001b]8;;\u0007\u001bXseven-bit SOS payload\u0007still SOS\u001b\\\u0098C1 SOS payload\u009c\u0000\n\tkept\u001bPprivate\u0007still private\u001b\\",
									provider_code: "provider\u009b31m-code\u001b[0m",
								},
								level: "error",
								message: "\u001b[1mProvider\u001b[0m\r diagnostic\u0000",
								observation_id: "process_diagnostic_1",
								raw: {
									engine_id: "codex",
									frame: "\u001b[31mprivate raw diagnostic\u001b[0m",
									transport: "test",
								},
								sequence: 1,
							},
							{
								occurred_at: "2026-08-12T00:00:01.000Z",
								run_id: "run_1",
								thread_id: "thread_1",
							},
						),
					);
					return yield* read_model.ReadSnapshot("thread_1");
				}),
			);

			expect(availability.status).toBe("available");
			if (availability.status !== "available") return;
			const diagnostic = availability.snapshot.items.find(
				(item) => item.type === "native_event",
			);
			expect(diagnostic).toMatchObject({
				error: {
					code: "AE-UNKNOWN-000",
					detail: "Native failure\n\tkept",
					provider_code: "provider-code",
				},
				summary: "Provider diagnostic",
				type: "native_event",
			});
			if (diagnostic?.type !== "native_event") return;
			const public_text = [
				diagnostic.summary,
				diagnostic.error?.detail,
				diagnostic.error?.provider_code,
			]
				.filter((value): value is string => value !== undefined)
				.join("\n");
			const public_diagnostic = JSON.stringify(diagnostic);
			expect(
				[...public_text].some((character) => {
					const code = character.charCodeAt(0);
					return (
						code <= 0x08 ||
						(code >= 0x0b && code <= 0x1f) ||
						(code >= 0x7f && code <= 0x9f)
					);
				}),
			).toBe(false);
			expect(public_text).not.toContain("SOS payload");
			expect(public_diagnostic).not.toContain("private raw diagnostic");
		} finally {
			await runtime.dispose();
		}
	});
});

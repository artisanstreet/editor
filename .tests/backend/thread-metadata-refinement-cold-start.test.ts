import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope } from "@artisan/protocol";
import {
	make_backend_runtime,
	make_thread_metadata_refiner_test_layer,
	ProtocolRouter,
	ThreadMetadataRefinementCoordinator,
	type ThreadMetadataRefinerInput,
} from "@artisan/backend";

import { JournalStore } from "../../modules/backend/src/persistence/journal-store";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-metadata-cold-start-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_command(message_id: string): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: { title: "Cold-start metadata", type: "thread.create" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-08-11T05:00:00.000Z",
		thread_id: "thread_metadata_cold_start",
	};
}

const AppendUserMessage = (journal: JournalStore["Service"], text: string, sequence: number) =>
	journal.AppendEvent({
		causation_id: `cold_start_cause_${sequence}`,
		correlation_id: `cold_start_correlation_${sequence}`,
		payload: {
			message_id: `cold_start_message_${sequence}`,
			reason: "no_active_run",
			text,
			type: "thread.message_queued",
			working_directory: "C:/workspace/artisan",
		},
		thread_id: "thread_metadata_cold_start",
	});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread metadata refinement cold start", () => {
	it("recovers only unfinished sources instead of rebuilding refined journal context", async () => {
		const database_path = await make_database_path();
		const historical_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			thread_metadata_refiner: make_thread_metadata_refiner_test_layer(() =>
				Effect.succeed({
					live_status: "Recovered",
				}),
			),
		});

		try {
			await historical_runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;

					yield* router.Route(make_command("create_metadata_cold_start"));

					for (const sequence of Array.from({ length: 20 }, (_, index) => index)) {
						yield* AppendUserMessage(journal, `already refined ${sequence}`, sequence);
						yield* coordinator.WaitForIdle;
					}
				}),
			);
		} finally {
			await historical_runtime.dispose();
		}

		const pending_runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			await pending_runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;

					yield* AppendUserMessage(journal, "recover only this source", 20);
				}),
			);
		} finally {
			await pending_runtime.dispose();
		}

		const recovered: ThreadMetadataRefinerInput[] = [];
		const recovery_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			thread_metadata_refiner: make_thread_metadata_refiner_test_layer((input) =>
				Effect.sync(() => {
					recovered.push(input);

					return {
						live_status: "Recovered",
					};
				}),
			),
		});

		try {
			await recovery_runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;

					yield* coordinator.WaitForIdle;
				}),
			);

			expect(recovered).toHaveLength(1);
			expect(recovered[0]!.recent_user_text).toEqual(["recover only this source"]);
		} finally {
			await recovery_runtime.dispose();
		}
	});
});

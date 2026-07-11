import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, ManagedRuntime, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { JournalStoreLive } from "../../modules/backend/src/persistence/journal-store";
import {
	GlobalGuidanceCanonical,
	GlobalGuidanceProviderSync,
	JournalCommands,
	JournalEvents,
} from "../../modules/backend/src/persistence/schema";
import {
	GlobalGuidanceRepository,
	GlobalGuidanceRepositoryLive,
} from "../../modules/backend/src/guidance/guidance-repository";
import { Database } from "../../modules/backend/src/persistence/database";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const hash_a = "a".repeat(64);
const hash_b = "b".repeat(64);

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-guidance-repository-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "guidance_repository_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed("2026-07-11T10:00:00.000Z"),
	});
}

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(),
		JournalNotifierLive,
	);
	const services = GlobalGuidanceRepositoryLive.pipe(
		Layer.provideMerge(JournalStoreLive),
		Layer.provideMerge(infrastructure),
	);

	return ManagedRuntime.make(services);
}

function make_update(message_id: string, content_hash = hash_a, byte_count = 24) {
	return {
		intent: {
			byte_count,
			content_hash,
			reason: "user_update" as const,
			type: "guidance.canonical.commit" as const,
		},
		message_id,
		origin: "frontend" as const,
		sent_at: "2026-07-11T10:00:00.000Z",
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("global guidance repository", () => {
	it("persists only metadata and exactly deduplicates sanitized update intents", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceRepository;
					const command = make_update("guidance_update_1");
					const initial = yield* guidance.Read;
					const accepted = yield* guidance.Accept(command);
					const duplicate = yield* guidance.Accept(command);
					const metadata = yield* guidance.Read;
					const commands = yield* database.client.select().from(JournalCommands);
					const events = yield* database.client.select().from(JournalEvents);

					return { accepted, commands, duplicate, events, initial, metadata };
				}),
			);

			expect(result.initial).toMatchObject({
				canonical: { status: "initialization_required" },
				providers: [],
			});
			expect(result.accepted).toMatchObject({ status: "accepted" });
			expect(result.duplicate).toMatchObject({ status: "duplicate" });
			expect(result.duplicate.event.journal_sequence).toBe(
				result.accepted.event.journal_sequence,
			);
			expect(result.metadata).toMatchObject({
				canonical: { byte_count: 24, content_hash: hash_a, status: "ready" },
			});
			expect(result.commands).toHaveLength(1);
			expect(result.events).toHaveLength(1);
			expect(result.events[0]).toMatchObject({
				stream_id: "settings:guidance",
				stream_sequence: 1,
				thread_id: "settings/guidance",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects conflicting retries without advancing canonical metadata", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceRepository;

					yield* guidance.Accept(make_update("guidance_update_conflict", hash_a, 24));
					const conflict = yield* guidance
						.Accept(make_update("guidance_update_conflict", hash_b, 48))
						.pipe(Effect.exit);

					return { conflict, metadata: yield* guidance.Read };
				}),
			);

			expect(result.conflict._tag).toBe("Failure");
			expect(result.metadata.canonical).toMatchObject({
				byte_count: 24,
				content_hash: hash_a,
				status: "ready",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("deduplicates semantic retries when only sent_at changes", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceRepository;
					const command = make_update("guidance_update_sent_at");
					const accepted = yield* guidance.Accept(command);
					const duplicate = yield* guidance.Accept({
						...command,
						sent_at: "2026-07-11T10:01:00.000Z",
					});

					return {
						accepted,
						commands: yield* database.client.select().from(JournalCommands),
						duplicate,
						events: yield* database.client.select().from(JournalEvents),
					};
				}),
			);

			expect(result.duplicate).toMatchObject({ status: "duplicate" });
			expect(result.duplicate.event.journal_sequence).toBe(
				result.accepted.event.journal_sequence,
			);
			expect(result.commands).toHaveLength(1);
			expect(result.events).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("survives restart and never writes guidance content to SQLite", async () => {
		const database_path = await make_database_path();
		const secret = "do-not-store-this-guidance-content";
		const first_runtime = make_runtime(database_path);

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceRepository;

					yield* guidance.Accept(
						make_update("guidance_restart_1", hash_a, secret.length),
					);
					yield* guidance.RecordProviderReconciliation({
						applied_byte_count: secret.length,
						applied_hash: hash_a,
						observed_byte_count: secret.length,
						observed_hash: hash_a,
						operation_id: "guidance_reconcile_1",
						provider: "codex",
						status: "synced",
					});
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(database_path);

		try {
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceRepository;
					const canonical = yield* database.client.select().from(GlobalGuidanceCanonical);
					const providers = yield* database.client
						.select()
						.from(GlobalGuidanceProviderSync);
					const commands = yield* database.client.select().from(JournalCommands);
					const events = yield* database.client.select().from(JournalEvents);

					return {
						metadata: yield* guidance.Read,
						persisted: JSON.stringify({ canonical, commands, events, providers }),
					};
				}),
			);

			expect(result.metadata.providers).toMatchObject([
				{ applied_hash: hash_a, provider: "codex", status: "synced" },
			]);
			expect(result.persisted).not.toContain(secret);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("deduplicates provider reconciliation and rejects reused operation ids", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceRepository;
					const input = {
						applied_byte_count: 24,
						applied_hash: hash_a,
						observed_byte_count: 24,
						observed_hash: hash_a,
						operation_id: "guidance_provider_sync_1",
						path: "C:/guidance/AGENTS.md",
						provider: "codex" as const,
						status: "synced" as const,
					};
					const accepted = yield* guidance.RecordProviderReconciliation(input);
					const duplicate = yield* guidance.RecordProviderReconciliation(input);
					const conflict = yield* guidance
						.RecordProviderReconciliation({
							...input,
							observed_hash: hash_b,
						})
						.pipe(Effect.exit);
					const events = yield* database.client.select().from(JournalEvents);

					return { accepted, conflict, duplicate, events };
				}),
			);

			expect(result.accepted.status).toBe("accepted");
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.duplicate.event.journal_sequence).toBe(
				result.accepted.event.journal_sequence,
			);
			expect(result.conflict._tag).toBe("Failure");
			expect(result.events).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("preflights private provider request fingerprints without exposing them", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceRepository;
					const operation_id = "guidance_provider_request_1";
					const initial = yield* guidance.PreflightProviderMutation({
						operation_id,
						request_fingerprint: hash_a,
					});
					const accepted = yield* guidance.RecordProviderReconciliation({
						applied_byte_count: 24,
						applied_hash: hash_a,
						observed_byte_count: 24,
						observed_hash: hash_a,
						operation_id,
						provider: "claude",
						request_fingerprint: hash_a,
						status: "synced",
					});
					const duplicate = yield* guidance.PreflightProviderMutation({
						operation_id,
						request_fingerprint: hash_a,
					});
					const changed_outcome = yield* guidance.RecordProviderReconciliation({
						last_error_code: "different_outcome",
						operation_id,
						provider: "claude",
						request_fingerprint: hash_a,
						status: "sync_failed",
					});
					const conflict = yield* guidance
						.PreflightProviderMutation({
							operation_id,
							request_fingerprint: hash_b,
						})
						.pipe(Effect.exit);
					const [command] = yield* database.client.select().from(JournalCommands);
					const [event] = yield* database.client.select().from(JournalEvents);
					const [provider] = yield* database.client
						.select()
						.from(GlobalGuidanceProviderSync);

					return {
						accepted,
						changed_outcome,
						command,
						conflict,
						duplicate,
						event,
						initial,
						provider,
					};
				}),
			);

			expect(Option.isNone(result.initial)).toBe(true);
			expect(Option.getOrThrow(result.duplicate).status).toBe("duplicate");
			expect(result.changed_outcome.status).toBe("duplicate");
			expect(result.changed_outcome.event.journal_sequence).toBe(
				result.accepted.event.journal_sequence,
			);
			expect(result.conflict._tag).toBe("Failure");
			expect(JSON.parse(result.command!.payload_json)).toMatchObject({
				request_fingerprint: hash_a,
				type: "guidance.provider.reconcile",
			});
			expect(result.event!.payload_json).not.toContain("request_fingerprint");
			expect(JSON.stringify(result.provider)).not.toContain("request_fingerprint");
		} finally {
			await runtime.dispose();
		}
	});

	it("reports corrupted provider reconciliation JSON as a durable invariant failure", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceRepository;
					const operation_id = "guidance_provider_corrupt_1";

					yield* guidance.RecordProviderReconciliation({
						operation_id,
						provider: "codex",
						request_fingerprint: hash_a,
						status: "synced",
					});
					yield* database.client.update(JournalCommands).set({ payload_json: "{" });

					return yield* guidance
						.PreflightProviderMutation({
							operation_id,
							request_fingerprint: hash_a,
						})
						.pipe(Effect.exit);
				}),
			);

			expect(result._tag).toBe("Failure");
			expect(JSON.stringify(result)).toContain("JournalInvariantError");
		} finally {
			await runtime.dispose();
		}
	});

	it("records selection ambiguity without storing candidate content", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceRepository;

					yield* guidance.Accept({
						intent: {
							candidate_hashes: [hash_a, hash_b],
							type: "guidance.selection.require",
						},
						message_id: "guidance_selection_required_1",
						origin: "backend",
						sent_at: "2026-07-11T10:00:00.000Z",
					});
					const [event] = yield* database.client.select().from(JournalEvents);

					return { event, metadata: yield* guidance.Read };
				}),
			);

			expect(result.metadata.canonical.status).toBe("selection_required");
			expect(result.event).toMatchObject({
				event_type: "guidance.selection.required",
				stream_id: "settings:guidance",
			});
		} finally {
			await runtime.dispose();
		}
	});
});

import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";

import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	JournalCommands,
	JournalEvents,
	ModelBehaviourProviderStates,
	ModelBehaviourSettings,
} from "../../modules/backend/src/persistence/tables";
import {
	ModelBehaviourRepository,
	ModelBehaviourRepositoryLive,
} from "../../modules/backend/src/model-behaviour/repository";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const hash_a = "a".repeat(64);
const hash_b = "b".repeat(64);

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-model-behaviour-repository-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "model_behaviour_repository_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed("2026-07-11T16:00:00.000Z"),
	});
}

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(),
		JournalNotifierLive,
	);
	const services = ModelBehaviourRepositoryLive.pipe(Layer.provideMerge(infrastructure));

	return ManagedRuntime.make(services);
}

function make_operation(message_id: string, request_fingerprint = hash_a) {
	return {
		message_id,
		origin: "frontend" as const,
		request_fingerprint,
		sent_at: "2026-07-11T16:00:00.000Z",
	};
}

function make_provider_state() {
	return {
		applied_hash: hash_a,
		native_key: "model_auto_compact_token_limit",
		observed_hash: hash_a,
		provider_id: "codex",
		setting_id: "auto_compaction_trigger_tokens" as const,
		status: "synced" as const,
		target_path: "C:/Users/Sander/.codex/config.toml",
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("model behaviour repository", () => {
	it("seeds the provider default, increments canonical version, and journals contiguous events", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ModelBehaviourRepository;
					const initial = yield* repository.Read;
					const preflight = yield* repository.Preflight(
						make_operation("model_behaviour_1"),
					);
					const accepted = yield* repository.Commit({
						operation: make_operation("model_behaviour_1"),
						provider_states: [
							make_provider_state(),
							{
								...make_provider_state(),
								provider_id: "claude",
								status: "unsupported",
							},
						],
						setting_update: {
							setting_id: "auto_compaction_trigger_tokens",
							value: { type: "integer", value: 32_768 },
						},
					});
					const persisted = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client
							.select()
							.from(ModelBehaviourProviderStates),
					};

					return {
						accepted,
						initial,
						persisted,
						preflight,
						read: yield* repository.Read,
					};
				}),
			);

			expect(result.initial.settings).toEqual([
				{
					setting_id: "auto_compaction_trigger_tokens",
					updated_at: "1970-01-01T00:00:00.000Z",
					value: { type: "provider_default" },
					version: 0,
				},
			]);
			expect(result.preflight).toEqual({ _tag: "Proceed" });
			expect(result.accepted).toMatchObject({ status: "accepted" });
			expect(result.accepted.events.map((event) => event.sequence)).toEqual([1, 2, 3]);
			expect(result.read.settings[0]).toMatchObject({
				value: { type: "integer", value: 32_768 },
				version: 1,
			});
			expect(result.read.provider_states).toHaveLength(2);
			const claude = result.persisted.providers.find(
				(provider) => provider.provider_id === "claude",
			);

			expect(claude).toEqual(
				expect.objectContaining({
					applied_hash: hash_a,
					native_key: "model_auto_compact_token_limit",
					observed_hash: hash_a,
					provider_id: "claude",
					setting_id: "auto_compaction_trigger_tokens",
					status: "unsupported",
					target_path: "C:/Users/Sander/.codex/config.toml",
				}),
			);
			expect(Object.keys(claude!)).toEqual([
				"provider_id",
				"setting_id",
				"status",
				"native_key",
				"target_path",
				"observed_hash",
				"applied_hash",
				"ignored_drift_hash",
				"backup_path",
				"last_error_code",
				"updated_at",
			]);
			expect(result.persisted.commands).toHaveLength(1);
			expect(result.persisted.events).toHaveLength(3);
		} finally {
			await runtime.dispose();
		}
	});

	it("deduplicates exact retries and rejects changed operation intent before mutation", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ModelBehaviourRepository;
					const input = {
						operation: make_operation("model_behaviour_retry"),
						provider_states: [make_provider_state()],
					};
					yield* repository.Commit(input);
					const duplicate = yield* repository.Commit(input);
					const preflight = yield* repository.Preflight(
						make_operation("model_behaviour_retry"),
					);
					const conflict = yield* repository
						.Commit({
							...input,
							operation: make_operation("model_behaviour_retry", hash_b),
						})
						.pipe(Effect.exit);

					return {
						commands: yield* database.client.select().from(JournalCommands),
						conflict,
						duplicate,
						events: yield* database.client.select().from(JournalEvents),
						preflight,
					};
				}),
			);

			expect(result.duplicate).toMatchObject({ status: "duplicate" });
			expect(result.preflight._tag).toBe("Duplicate");
			expect(result.duplicate.events).toEqual(
				result.preflight._tag === "Duplicate" ? result.preflight.events : [],
			);
			expect(result.conflict._tag).toBe("Failure");
			expect(JSON.stringify(result.conflict)).toContain("CommandIdConflict");
			expect(result.commands).toHaveLength(1);
			expect(result.events).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("survives restart and never persists adjacent provider config content", async () => {
		const database_path = await make_database_path();
		const fake_config = 'api_key = "secret-model-behaviour-config-value"';
		const observed_hash = createHash("sha256").update(fake_config).digest("hex");
		const first_runtime = make_runtime(database_path);

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* ModelBehaviourRepository;

					yield* repository.Commit({
						operation: make_operation("model_behaviour_restart"),
						provider_states: [{ ...make_provider_state(), observed_hash }],
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
					const repository = yield* ModelBehaviourRepository;
					const persisted = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client
							.select()
							.from(ModelBehaviourProviderStates),
						settings: yield* database.client.select().from(ModelBehaviourSettings),
					};

					return { persisted, read: yield* repository.Read };
				}),
			);

			expect(result.read.provider_states).toHaveLength(1);
			expect(JSON.stringify(result.persisted)).not.toContain(fake_config);
			expect(JSON.stringify(result.persisted)).not.toContain(
				"secret-model-behaviour-config-value",
			);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("rejects malformed persisted canonical JSON as a journal invariant failure", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ModelBehaviourRepository;

					yield* database.client.update(ModelBehaviourSettings).set({ value_json: "{" });

					return yield* repository.Read.pipe(Effect.exit);
				}),
			);

			expect(result._tag).toBe("Failure");
			expect(JSON.stringify(result)).toContain("JournalInvariantError");
		} finally {
			await runtime.dispose();
		}
	});
});

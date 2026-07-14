import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	EventStreams,
	ExternalWaitOperations,
	ExternalWaits,
	ExternalWaitWakeOutbox,
	JournalCommands,
	Projects,
	ThreadErasureClaims,
	ThreadTombstones,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { ThreadErasure, ThreadErasureLive } from "../../modules/backend/src/threads/thread-erasure";
import { ThreadResourceQuiescer } from "../../modules/backend/src/threads/thread-resource-quiescer";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const created_at = "2026-07-01T12:00:00.000Z";
const cutoff = "2026-07-08T12:00:00.000Z";
const deleted_at = "2026-07-14T12:00:00.000Z";

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-thread-erasure-external-wait-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		Layer.succeed(RuntimeMetadata, {
			instance_id: "thread_erasure_external_wait_test",
			MakeId: (prefix) => Effect.succeed(`${prefix}_external_wait_erasure`),
			Now: Effect.succeed(deleted_at),
		}),
		Layer.succeed(ThreadResourceQuiescer, { Quiesce: () => Effect.void }),
		JournalNotifierLive,
	);

	return ManagedRuntime.make(ThreadErasureLive.pipe(Layer.provideMerge(infrastructure)));
}

const SeedProject = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(Projects).values({
		canonical_root: "C:/artisan",
		display_name: "Artisan",
		project_id: "project_1",
		registered_at: created_at,
		updated_at: created_at,
		workspace_id: "workspace_1",
	});
});

const SeedWait = (input: {
	readonly state: "cancelled" | "suspended" | "waiting" | "wake_pending";
	readonly thread_id: string;
	readonly wait_id: string;
	readonly with_operation?: boolean;
	readonly with_outbox?: boolean;
}) =>
	Effect.gen(function* () {
		const database = yield* Database;
		const trigger = { _tag: "manual_resume" } as const;
		const state =
			input.state === "cancelled"
				? ({ _tag: "cancelled", reason: "user" } as const)
				: input.state === "suspended"
					? ({ _tag: "suspended", reason: "provider_unavailable" } as const)
					: input.state === "wake_pending"
						? ({ _tag: "wake_pending", trigger } as const)
						: ({ _tag: "waiting" } as const);
		const owner = {
			_tag: "thread_run" as const,
			agent_id: "agent_1",
			engine_id: "codex",
			run_id: `run_${input.wait_id}`,
		};
		const target = {
			branch: "main",
			expected_head_commit: "b".repeat(40),
			pull_request_number: 7,
			pull_request_origin: {
				native_id: "pr_7",
				provider_id: "github",
				resource_kind: "pull_request" as const,
			},
			repository: {
				host: "github.com",
				name: "editor",
				owner: "artisan",
				provider_id: "github",
			},
		};
		const snapshot = {
			baseline_fingerprint: "a".repeat(64),
			created_at,
			gates: [{ _tag: "required_checks_terminal" as const }],
			generation: 1,
			journal_sequence: 1,
			maximum_generation: 3,
			owner,
			project_id: "project_1",
			state,
			target,
			thread_id: input.thread_id,
			updated_at: created_at,
			version: 1,
			wait_id: input.wait_id,
			workspace_id: "workspace_1",
		};

		yield* database.client.insert(Threads).values({
			created_at,
			last_activity_at: created_at,
			primary_project_id: "project_1",
			primary_project_json: JSON.stringify({
				display_name: "Artisan",
				project_id: "project_1",
				root_path: "C:/artisan",
			}),
			thread_id: input.thread_id,
			title: "External wait",
			title_source: "initial",
			updated_at: created_at,
		});
		yield* database.client.insert(EventStreams).values({
			last_sequence: 0,
			stream_id: `thread:${input.thread_id}`,
		});
		yield* database.client.insert(ExternalWaits).values({
			baseline_fingerprint: snapshot.baseline_fingerprint,
			baseline_json: JSON.stringify({ private: "provider evidence" }),
			created_at,
			gates_json: JSON.stringify(snapshot.gates),
			generation: 1,
			journal_sequence: 1,
			maximum_generation: 3,
			next_observation_at: created_at,
			owner_json: JSON.stringify(owner),
			project_id: "project_1",
			request_fingerprint: "b".repeat(64),
			source_run_id: owner.run_id,
			state: input.state,
			state_json: JSON.stringify(state),
			target_json: JSON.stringify(target),
			thread_id: input.thread_id,
			timeout_at: deleted_at,
			updated_at: created_at,
			version: 1,
			wait_id: input.wait_id,
			workspace_id: "workspace_1",
		});

		if (input.with_outbox) {
			yield* database.client.insert(ExternalWaitWakeOutbox).values({
				created_at,
				follow_up_command_id: `follow_up_command_${input.wait_id}`,
				follow_up_run_id: `follow_up_run_${input.wait_id}`,
				outbox_id: `outbox_${input.wait_id}`,
				state: input.state === "cancelled" ? "cancelled" : "pending",
				trigger_fingerprint: "c".repeat(64),
				trigger_json: JSON.stringify(trigger),
				updated_at: created_at,
				wait_id: input.wait_id,
			});
		}

		if (input.with_operation) {
			yield* database.client.insert(JournalCommands).values({
				accepted_at: created_at,
				message_id: `command_${input.wait_id}`,
				origin: "frontend",
				payload_json: JSON.stringify({ type: "external_wait.command" }),
				payload_type: "external_wait.request",
				schema_version: 1,
				sent_at: created_at,
				status: "accepted",
				thread_id: input.thread_id,
			});
			yield* database.client.insert(ExternalWaitOperations).values({
				journal_sequence: 1,
				kind: "request",
				operation_id: `command_${input.wait_id}`,
				request_fingerprint: "b".repeat(64),
				result_snapshot_json: JSON.stringify(snapshot),
				sent_at: created_at,
				source_command_id: `command_${input.wait_id}`,
				thread_id: input.thread_id,
				wait_id: input.wait_id,
			});
		}
	});

afterEach(async () => {
	await Effect.runPromise(
		Effect.forEach(
			directories.splice(0),
			(directory) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(directory, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("ThreadErasure external waits", () => {
	it("erases terminal wait projections, replay rows, outbox state, and private baselines", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;

					yield* SeedProject;
					yield* SeedWait({
						state: "cancelled",
						thread_id: "thread_terminal",
						wait_id: "wait_terminal",
						with_operation: true,
						with_outbox: true,
					});
					const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

					return {
						commands: yield* database.client.select().from(JournalCommands),
						erased,
						operations: yield* database.client.select().from(ExternalWaitOperations),
						outbox: yield* database.client.select().from(ExternalWaitWakeOutbox),
						projects: yield* database.client.select().from(Projects),
						tombstones: yield* database.client.select().from(ThreadTombstones),
						waits: yield* database.client.select().from(ExternalWaits),
					};
				}),
			);

			expect(result.erased).toEqual(["thread_terminal"]);
			expect(result.waits).toEqual([]);
			expect(result.operations).toEqual([]);
			expect(result.outbox).toEqual([]);
			expect(result.commands).toEqual([]);
			expect(result.projects).toHaveLength(1);
			expect(result.tombstones).toMatchObject([{ thread_id: "thread_terminal" }]);
		} finally {
			await runtime.dispose();
		}
	});

	it("fences inactive-thread cleanup while a wait or wake remains active", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;

					yield* SeedProject;
					yield* SeedWait({
						state: "waiting",
						thread_id: "thread_waiting",
						wait_id: "wait_waiting",
					});
					yield* SeedWait({
						state: "wake_pending",
						thread_id: "thread_wake",
						wait_id: "wait_wake",
						with_outbox: true,
					});
					yield* SeedWait({
						state: "suspended",
						thread_id: "thread_suspended",
						wait_id: "wait_suspended",
					});
					const fenced = yield* erasure.CleanupExpired(cutoff, deleted_at);
					const claims_after_fence = yield* database.client
						.select()
						.from(ThreadErasureClaims);

					yield* database.client.update(ExternalWaits).set({
						state: "cancelled",
						state_json: JSON.stringify({ _tag: "cancelled", reason: "user" }),
					});
					yield* database.client
						.update(ExternalWaitWakeOutbox)
						.set({ state: "cancelled" });

					const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

					return {
						claims_after_fence,
						erased,
						fenced,
						threads: yield* database.client.select().from(Threads),
						waits: yield* database.client.select().from(ExternalWaits),
					};
				}),
			);

			expect(result.fenced).toEqual([]);
			expect(result.claims_after_fence).toEqual([]);
			expect([...result.erased].sort()).toEqual([
				"thread_suspended",
				"thread_waiting",
				"thread_wake",
			]);
			expect(result.threads).toEqual([]);
			expect(result.waits).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});
});

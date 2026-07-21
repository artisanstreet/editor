import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope } from "@artisan/protocol";
import { make_backend_runtime, ProjectionRebuildService, ProtocolRouter } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	Threads,
	GitMutationOperations,
	WorkspaceChangeOperations,
	WorkspaceChangeSnapshots,
	WorkspaceMutationAuthorities,
	WorkspaceMutationPayloads,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-transactional-acceptance-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_command(message_id = "command_create_1", title = "Transactional acceptance") {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: { title, type: "thread.create" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-18T12:00:00.000Z",
		thread_id: "thread_transactional_1",
	} satisfies CommandEnvelope;
}

async function route(runtime: ReturnType<typeof make_backend_runtime>, command: CommandEnvelope) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const router = yield* ProtocolRouter;

			return yield* router.Route(command);
		}),
	);
}

async function durable_state(runtime: ReturnType<typeof make_backend_runtime>) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const database = yield* Database;
			const [commands, events, streams, threads] = yield* Effect.all([
				database.client.select().from(JournalCommands),
				database.client.select().from(JournalEvents),
				database.client.select().from(EventStreams),
				database.client.select().from(Threads),
			]);

			return { commands, events, streams, threads };
		}),
	);
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("generic transactional command acceptance", () => {
	it("commits command dedupe, event/cursor, and thread projection together and replays an exact retry", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const first = await route(runtime, make_command());
			const after_first = await durable_state(runtime);
			const second = await route(runtime, make_command());
			const after_duplicate = await durable_state(runtime);

			expect(first).toMatchObject([
				{ kind: "command.receipt", payload: { status: "accepted" } },
				{ kind: "event", payload: { type: "thread.created" } },
			]);
			expect(second).toMatchObject([
				{ kind: "command.receipt", payload: { status: "duplicate" } },
				{ kind: "event", payload: { type: "thread.created" } },
			]);
			expect(
				after_first.commands.filter((command) => command.message_id === "command_create_1"),
			).toHaveLength(1);
			expect(
				after_first.events.filter((event) => event.correlation_id === "command_create_1"),
			).toHaveLength(1);
			expect(after_first.streams).toContainEqual(
				expect.objectContaining({
					last_sequence: 1,
					stream_id: "thread:thread_transactional_1",
				}),
			);
			expect(after_first.threads).toContainEqual(
				expect.objectContaining({
					thread_id: "thread_transactional_1",
					title: "Transactional acceptance",
				}),
			);
			expect(after_duplicate).toEqual(after_first);
		} finally {
			await runtime.dispose();
		}
	});

	it("deduplicates a semantic retry when only its transport timestamp changes", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const first = await route(runtime, make_command());
			const after_first = await durable_state(runtime);
			const duplicate = await route(runtime, {
				...make_command(),
				sent_at: "2026-07-18T12:00:01.000Z",
			});
			const after_duplicate = await durable_state(runtime);

			expect(first).toMatchObject([
				{ kind: "command.receipt", payload: { status: "accepted" } },
				{ kind: "event", payload: { type: "thread.created" } },
			]);
			expect(duplicate).toMatchObject([
				{ kind: "command.receipt", payload: { status: "duplicate" } },
				{ kind: "event", payload: { type: "thread.created" } },
			]);
			expect(after_duplicate).toEqual(after_first);
		} finally {
			await runtime.dispose();
		}
	});

	it("rolls back a rejected duplicate-intent command without creating a second command, event, cursor, or projection", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			await route(runtime, make_command());
			const before_rejection = await durable_state(runtime);
			const rejected = await route(
				runtime,
				make_command("command_create_2", "Conflicting create"),
			);
			const after_rejection = await durable_state(runtime);

			expect(rejected).toMatchObject([
				{
					kind: "command.receipt",
					payload: { error: { code: "thread.already_exists" }, status: "rejected" },
				},
			]);
			expect(after_rejection).toEqual(before_rejection);
		} finally {
			await runtime.dispose();
		}
	});

	it("preserves private mutation artifacts across rebuild and restart before an exact retry", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const hash_a = "a".repeat(64);
		const hash_b = "b".repeat(64);
		const timestamp = "2026-07-18T12:00:00.000Z";
		let before: unknown;

		try {
			await route(runtime, make_command());
			before = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const rebuild = yield* ProjectionRebuildService;
					yield* database.client.insert(WorkspaceChangeOperations).values({
						action: "replace",
						change_id: "restart_change_1",
						created_at: timestamp,
						lifecycle: "committed",
						message_id: "restart_workspace_command_1",
						request_fingerprint: hash_a,
						sent_at: timestamp,
						thread_id: "thread_transactional_1",
						updated_at: timestamp,
					});
					yield* database.client.insert(WorkspaceMutationPayloads).values({
						created_at: timestamp,
						expected: Buffer.from("before"),
						expected_byte_count: 6,
						expected_hash: hash_a,
						message_id: "restart_workspace_command_1",
						replacement: Buffer.from("after"),
						replacement_byte_count: 5,
						replacement_hash: hash_b,
						state: "available",
						thread_id: "thread_transactional_1",
						updated_at: timestamp,
					});
					yield* database.client.insert(WorkspaceChangeSnapshots).values({
						byte_count: 6,
						change_id: "restart_change_1",
						content: Buffer.from("before"),
						content_hash: hash_a,
						created_at: timestamp,
						state: "available",
						thread_id: "thread_transactional_1",
						updated_at: timestamp,
					});
					yield* database.client.insert(WorkspaceMutationAuthorities).values({
						agent_id: "agent_restart_1",
						approval: null,
						assignment_id: null,
						authority_kind: "base_run",
						change_id: "restart_change_1",
						created_at: timestamp,
						group_id: null,
						message_id: "restart_workspace_command_1",
						run_id: "run_restart_1",
						thread_id: "thread_transactional_1",
						working_directory: "C:/workspace",
						workspace_id: "workspace_restart_1",
					});
					yield* database.client.insert(GitMutationOperations).values({
						approval_id: "restart_git_approval_1",
						expected_snapshot_id: hash_a,
						expected_workspace_version: 1,
						kind: "stage",
						lifecycle: "succeeded",
						mutation_id: "restart_git_mutation_1",
						paths_json: '["src/example.ts"]',
						request_fingerprint: hash_b,
						requested_at: timestamp,
						result_snapshot_id: hash_b,
						result_workspace_version: 2,
						source_message_id: "restart_git_source_1",
						thread_id: "thread_transactional_1",
						updated_at: timestamp,
						workspace_id: "workspace_restart_1",
					});
					yield* rebuild.Rebuild();
					return {
						base: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							streams: yield* database.client.select().from(EventStreams),
							threads: yield* database.client.select().from(Threads),
						},
						git: yield* database.client.select().from(GitMutationOperations),
						payloads: yield* database.client.select().from(WorkspaceMutationPayloads),
						snapshots: yield* database.client.select().from(WorkspaceChangeSnapshots),
						authorities: yield* database.client
							.select()
							.from(WorkspaceMutationAuthorities),
					};
				}),
			);
		} finally {
			await runtime.dispose();
		}

		const restarted = make_backend_runtime({ database_path, migrations_path });
		try {
			const duplicate = await route(restarted, make_command());
			const after = await restarted.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						base: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							streams: yield* database.client.select().from(EventStreams),
							threads: yield* database.client.select().from(Threads),
						},
						git: yield* database.client.select().from(GitMutationOperations),
						payloads: yield* database.client.select().from(WorkspaceMutationPayloads),
						snapshots: yield* database.client.select().from(WorkspaceChangeSnapshots),
						authorities: yield* database.client
							.select()
							.from(WorkspaceMutationAuthorities),
					};
				}),
			);
			expect(duplicate[0]).toMatchObject({
				kind: "command.receipt",
				payload: { status: "duplicate" },
			});
			expect(after).toEqual(before);
		} finally {
			await restarted.dispose();
		}
	});
});

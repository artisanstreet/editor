import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { HelloEnvelope, SubscribeEnvelope, ThreadCreateEnvelope } from "@artisan/protocol";
import type { AuthoritativeCommandEnvelope } from "../../modules/backend/src/persistence/orchestration/message-command";
import {
	AgentOrchestrator,
	make_backend_runtime,
	ProtocolRouter,
	ProtocolServer,
} from "@artisan/backend";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-thread-identity-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_create_command(): AuthoritativeCommandEnvelope {
	return {
		kind: "command",
		message_id: "create_1",
		origin: "frontend",
		payload: {
			title: "Durable thread identity",
			type: "thread.create",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
		thread_id: "thread_1",
	};
}

function make_thread_command(
	message_id: string,
	payload: AuthoritativeCommandEnvelope["payload"],
	thread_id = "thread_1",
): AuthoritativeCommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
		thread_id,
	};
}

function make_thread_create_request(message_id: string, title: string): ThreadCreateEnvelope {
	return {
		kind: "thread.create.request",
		message_id,
		origin: "frontend",
		payload: { title },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
	};
}

function make_metadata_layer(now: { value: string }) {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "backend_thread_identity_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.sync(() => now.value),
	});
}

function make_hello(): HelloEnvelope {
	return {
		kind: "hello",
		message_id: "hello_1",
		origin: "frontend",
		payload: {
			event_cursors: [],
			last_journal_sequence: 0,
			supported_protocol_versions: [1],
		},
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
	};
}

function make_thread_subscribe(): SubscribeEnvelope {
	return {
		kind: "subscribe",
		message_id: "subscribe_1",
		origin: "frontend",
		payload: { type: "thread.list" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
		subscription_id: "thread_list_1",
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories.splice(0).map((directory) =>
			rm(directory, {
				force: true,
				recursive: true,
			}),
		),
	);
});

describe("thread identity", () => {
	it("migrates a fresh database with complete initial thread metadata", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const snapshot = await runtime.runPromise(
				Effect.gen(function* () {
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(make_create_command());

					return yield* threads.Snapshot();
				}),
			);
			const [thread] = snapshot.threads;

			expect(thread).toMatchObject({
				activity_version: 0,
				current_goal: "Durable thread identity",
				live_status: "Idle",
				metadata_version: 0,
				pinned: false,
				title: "Durable thread identity",
				title_locked: false,
				title_source: "initial",
			});
			expect(thread?.last_activity_at).toBe(thread?.created_at);
			expect(thread).not.toHaveProperty("archived_at");
			expect(thread).not.toHaveProperty("rename_suggestion");
		} finally {
			await runtime.dispose();
		}
	});

	it("locks a manual rename with exact durable retry and conflict behavior", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const rename = make_thread_command("rename_1", {
			title: "Retention owns erasure",
			type: "thread.rename",
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(make_create_command());
					const first = yield* router.Route(rename);
					const retry = yield* router.Route(rename);
					const [conflict] = yield* router.Route({
						...rename,
						payload: { title: "Different intent", type: "thread.rename" },
					});
					const snapshot = yield* threads.Snapshot();

					return { conflict, first, retry, snapshot };
				}),
			);
			const [first_receipt, first_event] = result.first;
			const [retry_receipt, retry_event] = result.retry;

			expect(first_receipt).toMatchObject({
				kind: "command.receipt",
				payload: { status: "accepted" },
			});
			expect(retry_receipt).toMatchObject({
				kind: "command.receipt",
				payload: { status: "duplicate" },
			});
			expect(retry_event).toEqual(first_event);
			expect(result.conflict).toMatchObject({
				kind: "command.receipt",
				payload: {
					error: { code: "command.id_conflict" },
					status: "rejected",
				},
			});
			expect(result.snapshot.threads[0]).toMatchObject({
				activity_version: 1,
				metadata_version: 1,
				title: "Retention owns erasure",
				title_locked: true,
				title_source: "manual",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects stale refinement and never overwrites a later manual title lock", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-10T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(make_create_command());
					now.value = "2026-07-11T18:00:00.000Z";
					yield* router.Route(
						make_thread_command("activity_1", {
							activity_kind: "user_message",
							type: "thread.activity.record",
						}),
					);
					const stale = yield* router.Route(
						make_thread_command("refine_stale", {
							basis_activity_version: 0,
							basis_metadata_version: 0,
							current_goal: "Old goal",
							live_status: "Old status",
							title: "Old title",
							type: "thread.metadata.refine",
						}),
					);
					yield* router.Route(
						make_thread_command("refine_current", {
							basis_activity_version: 1,
							basis_metadata_version: 0,
							current_goal: "Implement retention erasure",
							live_status: "Designing erasure...",
							rename_suggestion: "Durable retention",
							title: "Thread retention",
							type: "thread.metadata.refine",
						}),
					);
					now.value = "2026-07-12T18:00:00.000Z";
					yield* router.Route(
						make_thread_command("rename_lock", {
							title: "Sacred manual title",
							type: "thread.rename",
						}),
					);
					yield* router.Route(
						make_thread_command("refine_locked", {
							basis_activity_version: 2,
							basis_metadata_version: 2,
							current_goal: "Verify cleanup recovery",
							live_status: "Running retention tests...",
							rename_suggestion: "Cleanup recovery",
							title: "Forbidden automatic title",
							type: "thread.metadata.refine",
						}),
					);

					return { snapshot: yield* threads.Snapshot(), stale };
				}),
			);

			expect(result.stale[1]).toMatchObject({
				kind: "event",
				payload: {
					basis_activity_version: 0,
					basis_metadata_version: 0,
					type: "thread.refinement.ignored",
				},
			});
			expect(result.snapshot.threads[0]).toMatchObject({
				activity_version: 2,
				current_goal: "Verify cleanup recovery",
				last_activity_at: "2026-07-12T18:00:00.000Z",
				live_status: "Running retention tests...",
				metadata_version: 3,
				rename_suggestion: "Cleanup recovery",
				title: "Sacred manual title",
				title_locked: true,
				title_source: "manual",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("advances activity monotonically across pin and archive lifecycle changes", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-12T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const snapshots = await runtime.runPromise(
				Effect.gen(function* () {
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;
					const read = threads
						.Snapshot()
						.pipe(Effect.map((snapshot) => snapshot.threads[0]!));

					yield* router.Route(make_create_command());
					now.value = "2026-07-13T18:00:00.000Z";
					yield* router.Route(
						make_thread_command("activity_forward", {
							activity_kind: "user_message",
							type: "thread.activity.record",
						}),
					);
					now.value = "2026-07-11T18:00:00.000Z";
					yield* router.Route(
						make_thread_command("activity_backward", {
							activity_kind: "run_started",
							type: "thread.activity.record",
						}),
					);
					const monotonic = yield* read;
					now.value = "2026-07-14T18:00:00.000Z";
					yield* router.Route(make_thread_command("pin_1", { type: "thread.pin" }));
					const pinned = yield* read;
					now.value = "2026-07-15T18:00:00.000Z";
					yield* router.Route(
						make_thread_command("archive_1", { type: "thread.archive" }),
					);
					const archived = yield* read;
					now.value = "2026-07-16T18:00:00.000Z";
					yield* router.Route(
						make_thread_command("restore_1", { type: "thread.restore" }),
					);
					const restored = yield* read;
					now.value = "2026-07-17T18:00:00.000Z";
					yield* router.Route(make_thread_command("unpin_1", { type: "thread.unpin" }));

					return { archived, final: yield* read, monotonic, pinned, restored };
				}),
			);

			expect(snapshots.monotonic).toMatchObject({
				activity_version: 2,
				last_activity_at: "2026-07-13T18:00:00.000Z",
				metadata_version: 0,
			});
			expect(snapshots.pinned).toMatchObject({
				activity_version: 3,
				metadata_version: 1,
				pinned: true,
			});
			expect(snapshots.archived).toMatchObject({
				activity_version: 4,
				archived_at: "2026-07-15T18:00:00.000Z",
			});
			expect(snapshots.restored).not.toHaveProperty("archived_at");
			expect(snapshots.final).toMatchObject({
				activity_version: 6,
				metadata_version: 4,
				pinned: false,
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("records user messages and run transitions through the normal service path", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-10T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const thread = await runtime.runPromise(
				Effect.gen(function* () {
					const orchestrator = yield* AgentOrchestrator;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(make_create_command());
					now.value = "2026-07-11T18:00:00.000Z";
					yield* orchestrator.Handle(
						make_thread_command("message_1", {
							engine_id: "missing_engine",
							text: "Please implement durable erasure",
							type: "thread.send_message",
							working_directory: "C:/workspace",
						}),
					);

					return (yield* threads.Snapshot()).threads[0]!;
				}),
			);

			expect(thread.activity_version).toBeGreaterThanOrEqual(2);
			expect(thread.last_activity_at).toBe("2026-07-11T18:00:00.000Z");
		} finally {
			await runtime.dispose();
		}
	});

	it("publishes complete sidebar metadata through thread-list subscriptions", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-10T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const patches = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;

						yield* connection.Receive(make_hello());
						yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);
						yield* connection.Receive(make_thread_subscribe());
						yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);
						yield* connection.Receive(
							make_thread_create_request(
								"create_subscription",
								"Durable thread identity",
							),
						);
						const created = yield* connection.Outbound.pipe(
							Stream.take(3),
							Stream.runCollect,
						);
						const created_thread = created.find(
							(envelope) => envelope.kind === "thread.create.result",
						);
						if (created_thread?.kind !== "thread.create.result") {
							return yield* Effect.die("Forge did not return the created thread");
						}
						now.value = "2026-07-11T18:00:00.000Z";
						yield* connection.Receive(
							make_thread_command(
								"rename_subscription",
								{
									title: "Locked sidebar title",
									type: "thread.rename",
								},
								created_thread.payload.thread_id,
							),
						);
						const renamed = yield* connection.Outbound.pipe(
							Stream.take(3),
							Stream.runCollect,
						);

						return {
							created: created.find(
								(envelope) => envelope.kind === "thread.list.upsert",
							),
							renamed: renamed.find(
								(envelope) => envelope.kind === "thread.list.upsert",
							),
						};
					}),
				),
			);

			expect(patches.created).toMatchObject({
				payload: {
					activity_version: 0,
					live_status: "Idle",
					metadata_version: 0,
					title_locked: false,
					title_source: "initial",
				},
			});
			expect(patches.renamed).toMatchObject({
				payload: {
					activity_version: 1,
					metadata_version: 1,
					title: "Locked sidebar title",
					title_locked: true,
					title_source: "manual",
				},
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("publishes live sidebar activity for queued messages and run transitions", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-10T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const orchestrator = yield* AgentOrchestrator;
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;

						yield* connection.Receive(make_hello());
						yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);
						yield* connection.Receive(make_thread_subscribe());
						yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);
						yield* connection.Receive(
							make_thread_create_request(
								"create_activity",
								"Durable thread identity",
							),
						);
						const created = yield* connection.Outbound.pipe(
							Stream.take(3),
							Stream.runCollect,
						);
						const created_thread = created.find(
							(envelope) => envelope.kind === "thread.create.result",
						);
						if (created_thread?.kind !== "thread.create.result") {
							return yield* Effect.die("Forge did not return the created thread");
						}

						now.value = "2026-07-11T18:00:00.000Z";
						yield* orchestrator.Handle(
							make_thread_command(
								"message_subscription",
								{
									engine_id: "missing_engine",
									text: "Advance the live sidebar activity",
									type: "thread.send_message",
									working_directory: "C:/workspace",
								},
								created_thread.payload.thread_id,
							),
						);

						return {
							envelopes: yield* connection.Outbound.pipe(
								Stream.take(5),
								Stream.runCollect,
							),
							thread_id: created_thread.payload.thread_id,
						};
					}),
				),
			);
			const upserts = output.envelopes.filter(
				(envelope) => envelope.kind === "thread.list.upsert",
			);

			expect(output.envelopes.map((envelope) => envelope.kind)).toEqual([
				"event",
				"thread.list.upsert",
				"event",
				"event",
				"thread.list.upsert",
			]);
			const activity_versions = upserts.map((envelope) => envelope.payload.activity_version);

			expect(activity_versions).toHaveLength(2);
			expect(activity_versions[0]).toBeGreaterThanOrEqual(2);
			expect(activity_versions[1]).toBeGreaterThanOrEqual(activity_versions[0]!);
			expect(upserts.at(-1)).toMatchObject({
				payload: {
					last_activity_at: "2026-07-11T18:00:00.000Z",
					thread_id: output.thread_id,
				},
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("publishes live sidebar activity for terminal, process, file, and diff events", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-10T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const updates = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const journal = yield* JournalStore;
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;

						yield* connection.Receive(make_hello());
						yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);
						yield* connection.Receive(make_thread_subscribe());
						yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);
						yield* connection.Receive(
							make_thread_create_request(
								"create_resources",
								"Durable thread identity",
							),
						);
						const created = yield* connection.Outbound.pipe(
							Stream.take(3),
							Stream.runCollect,
						);
						const created_thread = created.find(
							(envelope) => envelope.kind === "thread.create.result",
						);
						if (created_thread?.kind !== "thread.create.result") {
							return yield* Effect.die("Forge did not return the created thread");
						}
						const thread_id = created_thread.payload.thread_id;

						now.value = "2026-07-11T18:00:00.000Z";
						yield* journal.AppendEvent({
							causation_id: "terminal_cause",
							correlation_id: "terminal_correlation",
							payload: {
								action: "opened",
								terminal: {
									args: [],
									cols: 80,
									created_at: now.value,
									executable: "pwsh",
									generation: 1,
									rows: 24,
									state: "active",
									terminal_id: "terminal_1",
									thread_id,
									updated_at: now.value,
									workspace_id: "workspace_1",
									working_directory: "C:/workspace",
								},
								type: "terminal.lifecycle",
							},
							thread_id,
						});
						const terminal = yield* connection.Outbound.pipe(
							Stream.take(2),
							Stream.runCollect,
						);

						yield* journal.AppendEvent({
							causation_id: "file_cause",
							correlation_id: "file_correlation",
							payload: {
								artifact: {
									artifact_id: "file_1",
									assignment_id: "assignment_1",
									created_at: now.value,
									group_id: "group_1",
									kind: "file",
									label: "Changed file",
									run_id: "run_1",
									uri: "file:///C:/workspace/changed.ts",
								},
								group_id: "group_1",
								type: "artifact.recorded",
							},
							thread_id,
						});
						const file = yield* connection.Outbound.pipe(
							Stream.take(2),
							Stream.runCollect,
						);

						yield* journal.AppendEvent({
							causation_id: "diff_cause",
							correlation_id: "diff_correlation",
							payload: {
								artifact: {
									artifact_id: "diff_1",
									assignment_id: "assignment_1",
									content: "diff content",
									created_at: now.value,
									group_id: "group_1",
									kind: "diff",
									label: "Changed diff",
									run_id: "run_1",
								},
								group_id: "group_1",
								type: "artifact.recorded",
							},
							thread_id,
						});
						const diff = yield* connection.Outbound.pipe(
							Stream.take(2),
							Stream.runCollect,
						);

						yield* connection.Receive(
							make_thread_command(
								"process_activity",
								{
									activity_kind: "process_attached",
									type: "thread.activity.record",
								},
								thread_id,
							),
						);
						const process = yield* connection.Outbound.pipe(
							Stream.take(3),
							Stream.runCollect,
						);

						return [
							terminal.find((envelope) => envelope.kind === "thread.list.upsert"),
							file.find((envelope) => envelope.kind === "thread.list.upsert"),
							diff.find((envelope) => envelope.kind === "thread.list.upsert"),
							process.find((envelope) => envelope.kind === "thread.list.upsert"),
						];
					}),
				),
			);

			expect(updates).toMatchObject([
				{ payload: { activity_version: 1 }, sequence: 2 },
				{ payload: { activity_version: 2 }, sequence: 3 },
				{ payload: { activity_version: 3 }, sequence: 4 },
				{ payload: { activity_version: 4 }, sequence: 5 },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("ignores an older refinement that completes after a newer intent", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-10T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(make_create_command());
					yield* router.Route(
						make_thread_command("activity_old_basis", {
							activity_kind: "user_message",
							type: "thread.activity.record",
						}),
					);
					yield* router.Route(
						make_thread_command("activity_new_basis", {
							activity_kind: "diff_attached",
							type: "thread.activity.record",
						}),
					);
					yield* router.Route(
						make_thread_command("refinement_newer", {
							basis_activity_version: 2,
							basis_metadata_version: 0,
							current_goal: "Keep the newer goal",
							live_status: "Applying the newer result...",
							title: "Newer thread identity",
							type: "thread.metadata.refine",
						}),
					);
					const late = yield* router.Route(
						make_thread_command("refinement_older_late", {
							basis_activity_version: 1,
							basis_metadata_version: 0,
							current_goal: "Overwrite with old goal",
							live_status: "Old result arrived",
							title: "Older thread identity",
							type: "thread.metadata.refine",
						}),
					);

					return { late, snapshot: yield* threads.Snapshot() };
				}),
			);

			expect(result.late[1]).toMatchObject({
				payload: {
					basis_activity_version: 1,
					basis_metadata_version: 0,
					type: "thread.refinement.ignored",
				},
			});
			expect(result.snapshot.threads[0]).toMatchObject({
				current_goal: "Keep the newer goal",
				live_status: "Applying the newer result...",
				metadata_version: 1,
				title: "Newer thread identity",
				title_source: "automatic",
			});
		} finally {
			await runtime.dispose();
		}
	});
});

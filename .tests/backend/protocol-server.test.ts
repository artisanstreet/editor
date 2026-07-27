import { createHash } from "node:crypto";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Fiber, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	AckEnvelope,
	ContentIdentity,
	HelloEnvelope,
	OutboundControlEnvelope,
	SubscribeEnvelope,
	ThreadCreateEnvelope,
	ThreadListQueryEnvelope,
	WorkspaceChangeDiffQueryEnvelope,
	WorkspaceChangeListQueryEnvelope,
	WorkspaceChangeReviewEnvelope,
	WorkspaceChangeRollbackEnvelope,
	WorkspaceFileReadQueryEnvelope,
	WorkspaceFileReplaceEnvelope,
} from "@artisan/protocol";
import {
	DecodeProtocolConnectionOptions,
	make_backend_runtime,
	ProtocolServer,
	type ProtocolConnection,
} from "@artisan/backend";

import { ProtocolRouter } from "../../modules/backend/src/protocol/protocol-router";
import { Database } from "../../modules/backend/src/persistence/database";
import {
	OrchestrationCoordinators,
	OrchestrationRuns,
	WorkspaceChanges,
} from "../../modules/backend/src/persistence/schema";
import { MakeNodeTestWorkspaceBoundedRegularFileStoreRegistryLayer } from "./bounded-regular-file-store-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const temporary_directories: Array<string> = [];
const workspace_time = "2026-07-12T14:00:00.000Z";
const workspace_encoder = new TextEncoder();

function content_identity(content: string): ContentIdentity {
	const bytes = workspace_encoder.encode(content);

	return {
		algorithm: "sha256",
		byte_count: bytes.byteLength,
		content_hash: createHash("sha256").update(bytes).digest("hex"),
	};
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-editor-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

async function make_workspace_runtime() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-editor-workspace-protocol-"));
	const root = join(directory, "workspace");
	const database_path = join(directory, "artisan.db");

	temporary_directories.push(directory);
	await mkdir(join(root, "src"), { recursive: true });
	await writeFile(join(root, "src", "example.ts"), "before");

	return {
		database_path,
		root,
		runtime: make_backend_runtime({
			database_path,
			migrations_path,
			workspace_bounded_regular_file_store_registry:
				MakeNodeTestWorkspaceBoundedRegularFileStoreRegistryLayer([
					{ root, workspace_id: "workspace_protocol" },
				]),
		}),
	};
}

function workspace_replace(
	message_id: string,
	change_id: string,
	content: string,
	expected_before: ContentIdentity = content_identity("before"),
): WorkspaceFileReplaceEnvelope {
	return {
		agent_id: "agent_workspace_protocol",
		kind: "workspace.file.replace",
		message_id,
		origin: "frontend",
		payload: {
			change_id,
			content,
			expected_before,
			path: "src/example.ts",
			workspace_id: "workspace_protocol",
		},
		protocol_version: 1,
		raw_origin: { provider: "codex", reference: "native_workspace_protocol" },
		run_id: "run_workspace_protocol",
		schema_version: 1,
		sent_at: workspace_time,
		thread_id: "thread_workspace_protocol",
	};
}

function workspace_read(message_id: string): WorkspaceFileReadQueryEnvelope {
	return {
		kind: "workspace.file.read.query",
		message_id,
		origin: "frontend",
		payload: { path: "src/example.ts", workspace_id: "workspace_protocol" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
	};
}

function workspace_list(message_id: string): WorkspaceChangeListQueryEnvelope {
	return {
		kind: "workspace.change.list.query",
		message_id,
		origin: "frontend",
		payload: { thread_id: "thread_workspace_protocol", workspace_id: "workspace_protocol" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
	};
}

function workspace_diff(
	message_id: string,
	change_id = "change_workspace_protocol",
): WorkspaceChangeDiffQueryEnvelope {
	return {
		kind: "workspace.change.diff.query",
		message_id,
		origin: "frontend",
		payload: { change_id, thread_id: "thread_workspace_protocol" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
	};
}

function workspace_review(
	message_id: string,
	change_id = "change_workspace_protocol",
): WorkspaceChangeReviewEnvelope {
	return {
		kind: "workspace.change.review",
		message_id,
		origin: "frontend",
		payload: { change_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
		thread_id: "thread_workspace_protocol",
	};
}

function workspace_rollback(message_id: string): WorkspaceChangeRollbackEnvelope {
	return {
		kind: "workspace.change.rollback",
		message_id,
		origin: "frontend",
		payload: {
			change_id: "change_workspace_protocol",
			expected_after: content_identity("after"),
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
		thread_id: "thread_workspace_protocol",
	};
}

function seed_workspace_run(root: string) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const router = yield* ProtocolRouter;

		yield* router.Route({
			kind: "command",
			message_id: "create_workspace_protocol_thread",
			origin: "frontend",
			payload: { title: "Workspace protocol", type: "thread.create" },
			protocol_version: 1,
			schema_version: 1,
			sent_at: workspace_time,
			thread_id: "thread_workspace_protocol",
		});
		yield* database.client.insert(OrchestrationCoordinators).values({
			active_run_id: "run_workspace_protocol",
			agent_id: "agent_workspace_protocol",
			created_at: workspace_time,
			display_name: "Coordinator",
			engine_id: "engine_workspace_protocol",
			role: "primary",
			thread_id: "thread_workspace_protocol",
			updated_at: workspace_time,
		});
		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: "agent_workspace_protocol",
			created_at: workspace_time,
			engine_id: "engine_workspace_protocol",
			run_id: "run_workspace_protocol",
			status: "running",
			thread_id: "thread_workspace_protocol",
			updated_at: workspace_time,
			working_directory: root,
		});
	});
}

function terminalize_workspace_run() {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client
			.update(OrchestrationRuns)
			.set({ status: "complete", updated_at: workspace_time });
	});
}

function make_hello(
	message_id = "hello_1",
	last_journal_sequence = 0,
	event_cursors: HelloEnvelope["payload"]["event_cursors"] = [],
	supported_protocol_versions: HelloEnvelope["payload"]["supported_protocol_versions"] = [1],
	resume_mode: NonNullable<HelloEnvelope["payload"]["resume_mode"]> = "fresh",
): HelloEnvelope {
	return {
		kind: "hello",
		message_id,
		origin: "frontend",
		payload: {
			event_cursors,
			last_journal_sequence,
			resume_mode,
			supported_protocol_versions,
		},
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function make_thread_create(
	message_id = "command_1",
	title = "Protocol integration",
): ThreadCreateEnvelope {
	return {
		kind: "thread.create.request",
		message_id,
		origin: "frontend",
		payload: { title },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function created_thread_id(envelopes: Iterable<OutboundControlEnvelope>) {
	const result = [...envelopes].find((envelope) => envelope.kind === "thread.create.result");

	if (result?.kind !== "thread.create.result") {
		throw new Error("Forge did not return a created thread projection");
	}

	return result.payload.thread_id;
}

function make_query(message_id = "query_1"): ThreadListQueryEnvelope {
	return {
		kind: "thread.list.query",
		message_id,
		origin: "frontend",
		payload: {},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function make_subscribe(message_id = "subscribe_1"): SubscribeEnvelope {
	return {
		kind: "subscribe",
		message_id,
		origin: "frontend",
		payload: { type: "thread.list" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		subscription_id: "subscription_1",
	};
}

function make_ack(
	message_id: string,
	journal_sequence: number,
	event_cursors: AckEnvelope["payload"]["event_cursors"],
): AckEnvelope {
	return {
		kind: "ack",
		message_id,
		origin: "frontend",
		payload: { event_cursors, journal_sequence },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

const open_connection = Effect.gen(function* () {
	const protocol_server = yield* ProtocolServer;

	return yield* protocol_server.Open;
});

function take_outbound(connection: ProtocolConnection, count: number) {
	return connection.Outbound.pipe(Stream.take(count), Stream.runCollect);
}

function take_until_outbound(
	connection: ProtocolConnection,
	predicate: (envelope: OutboundControlEnvelope) => boolean,
) {
	return connection.Outbound.pipe(Stream.takeUntil(predicate), Stream.runCollect);
}

const negotiate = (connection: ProtocolConnection, hello = make_hello()) =>
	Effect.gen(function* () {
		yield* connection.Receive(hello);

		return yield* take_until_outbound(
			connection,
			(envelope) => envelope.kind === "replay.complete",
		);
	});

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

describe("protocol server", () => {
	it("replays a Forge-owned create request with its original durable thread id", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const request: ThreadCreateEnvelope = {
			kind: "thread.create.request",
			message_id: "forge_create_retry",
			origin: "frontend",
			payload: { title: "Retry-safe Forge identity" },
			protocol_version: 1,
			schema_version: 1,
			sent_at: workspace_time,
		};

		try {
			const first = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;
						yield* negotiate(connection);
						yield* connection.Receive(request);
						return yield* take_until_outbound(
							connection,
							(envelope) => envelope.kind === "thread.create.result",
						);
					}),
				),
			);
			const replay = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;
						yield* negotiate(connection);
						yield* connection.Receive(request);
						return yield* take_until_outbound(
							connection,
							(envelope) => envelope.kind === "thread.create.result",
						);
					}),
				),
			);
			const first_result = [...first].find(
				(envelope) => envelope.kind === "thread.create.result",
			);
			const replay_result = [...replay].find(
				(envelope) => envelope.kind === "thread.create.result",
			);

			expect(first_result?.payload.thread_id).toMatch(/^thread_\d+$/);
			expect(replay_result?.payload.thread_id).toBe(first_result?.payload.thread_id);
		} finally {
			await runtime.dispose();
		}
	});

	it("routes controlled workspace queries and mutations through correlated receipts", async () => {
		const { root, runtime } = await make_workspace_runtime();

		try {
			await runtime.runPromise(seed_workspace_run(root));
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive(workspace_read("workspace_read"));
						const read = yield* take_outbound(connection, 1);

						yield* connection.Receive(workspace_list("workspace_list_empty"));
						const empty_list = yield* take_outbound(connection, 1);
						yield* connection.Receive(
							workspace_diff("workspace_diff_missing", "change_workspace_missing"),
						);
						const diff_missing = yield* take_outbound(connection, 1);

						yield* Effect.promise(() =>
							writeFile(join(root, "src", "example.ts"), "external"),
						);
						yield* connection.Receive(
							workspace_replace(
								"workspace_replace_conflict",
								"change_workspace_conflict",
								"after",
							),
						);
						const conflict = yield* take_outbound(connection, 2);
						yield* connection.Receive(
							workspace_review(
								"workspace_review_unavailable",
								"change_workspace_missing",
							),
						);
						const review_unavailable = yield* take_outbound(connection, 1);

						yield* Effect.promise(() =>
							writeFile(join(root, "src", "example.ts"), "before"),
						);
						yield* connection.Receive(
							workspace_replace(
								"workspace_replace",
								"change_workspace_protocol",
								"after",
							),
						);
						const replace = yield* take_outbound(connection, 2);

						yield* connection.Receive(
							workspace_replace(
								"workspace_replace",
								"change_workspace_protocol",
								"after",
							),
						);
						const replace_duplicate = yield* take_outbound(connection, 1);

						yield* connection.Receive(workspace_list("workspace_list_recorded"));
						const recorded_list = yield* take_outbound(connection, 1);
						yield* connection.Receive(workspace_diff("workspace_diff_recorded"));
						const recorded_diff = yield* take_outbound(connection, 1);

						yield* terminalize_workspace_run();
						yield* connection.Receive(workspace_review("workspace_review"));
						const review = yield* take_outbound(connection, 2);

						yield* connection.Receive(workspace_review("workspace_review"));
						const review_duplicate = yield* take_outbound(connection, 1);

						yield* connection.Receive(workspace_rollback("workspace_rollback"));
						const rollback = yield* take_outbound(connection, 2);

						yield* connection.Receive(workspace_rollback("workspace_rollback"));
						const rollback_duplicate = yield* take_outbound(connection, 1);

						yield* connection.Receive({
							...workspace_read("workspace_read_unavailable"),
							payload: { path: "src/example.ts", workspace_id: "missing_workspace" },
						});
						const read_unavailable = yield* take_outbound(connection, 1);
						const database = yield* Database;

						yield* database.client.insert(WorkspaceChanges).values({
							after_identity_json: "{}",
							agent_id: "agent_workspace_protocol",
							before_identity_json: "{}",
							change_id: "change_workspace_invalid",
							created_at: workspace_time,
							path: "src/example.ts",
							raw_origin_json: null,
							review_state: "rolled_back",
							reviewed_at: workspace_time,
							rollback_state: "consumed",
							rolled_back_at: workspace_time,
							run_id: "run_workspace_protocol",
							source_command_id: "workspace_invalid_command",
							thread_id: "thread_workspace_protocol",
							updated_at: workspace_time,
							version: 1,
							workspace_id: "workspace_protocol",
						});
						yield* connection.Receive(workspace_list("workspace_list_unavailable"));
						const list_unavailable = yield* take_outbound(connection, 1);

						return {
							conflict,
							diff_missing,
							empty_list,
							list_unavailable,
							read,
							read_unavailable,
							recorded_diff,
							recorded_list,
							replace,
							replace_duplicate,
							review,
							review_duplicate,
							review_unavailable,
							rollback,
							rollback_duplicate,
						};
					}),
				),
			);

			expect(output.read).toMatchObject([
				{
					correlation_id: "workspace_read",
					kind: "workspace.file.read.query.result",
					payload: { content: "before", identity: content_identity("before") },
				},
			]);
			expect(output.empty_list).toMatchObject([
				{
					correlation_id: "workspace_list_empty",
					kind: "workspace.change.list.query.result",
					payload: { changes: [] },
				},
			]);
			expect(output.diff_missing).toMatchObject([
				{
					correlation_id: "workspace_diff_missing",
					kind: "protocol.error",
					payload: { code: "workspace.diff_unavailable", retryable: false },
				},
			]);
			expect(output.conflict).toMatchObject([
				{
					causation_id: "workspace_replace_conflict",
					correlation_id: "workspace_replace_conflict",
					kind: "command.receipt",
					payload: {
						error: { code: "workspace.conflict", retryable: false },
						status: "rejected",
					},
					thread_id: "thread_workspace_protocol",
				},
				{
					correlation_id: "workspace_replace_conflict",
					kind: "event",
					payload: {
						action: "recorded",
						conflict: { resolution: "user_action_required" },
						type: "workspace.conflict.updated",
					},
				},
			]);
			expect(output.replace).toMatchObject([
				{
					causation_id: "workspace_replace",
					correlation_id: "workspace_replace",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "accepted" },
				},
				{
					agent_id: "agent_workspace_protocol",
					correlation_id: "workspace_replace",
					kind: "event",
					payload: {
						action: "recorded",
						change: {
							raw_origin: {
								provider: "codex",
								reference: "native_workspace_protocol",
							},
						},
						type: "workspace.change.updated",
					},
					raw_origin: { provider: "codex", reference: "native_workspace_protocol" },
					run_id: "run_workspace_protocol",
					thread_id: "thread_workspace_protocol",
				},
			]);
			expect(output.replace_duplicate).toMatchObject([
				{
					correlation_id: "workspace_replace",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "duplicate" },
				},
			]);
			expect(output.recorded_list).toMatchObject([
				{
					correlation_id: "workspace_list_recorded",
					kind: "workspace.change.list.query.result",
					payload: {
						changes: [
							{
								change_id: "change_workspace_protocol",
								review_state: "needs_review",
							},
						],
					},
				},
			]);
			expect(output.recorded_diff).toMatchObject([
				{
					correlation_id: "workspace_diff_recorded",
					kind: "workspace.change.diff.query.result",
					payload: {
						added_line_count: 1,
						after_identity: content_identity("after"),
						before_identity: content_identity("before"),
						change_id: "change_workspace_protocol",
						context_lines: 3,
						format: "unified",
						format_version: 1,
						patch: expect.stringContaining(
							"--- a/src/example.ts\n+++ b/src/example.ts\n",
						),
						patch_identity: {
							algorithm: "sha256",
							byte_count: expect.any(Number),
							content_hash: expect.stringMatching(/^[a-f0-9]{64}$/),
						},
						removed_line_count: 1,
						truncated: false,
						workspace_id: "workspace_protocol",
					},
				},
			]);
			expect(output.review).toMatchObject([
				{
					correlation_id: "workspace_review",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "accepted" },
				},
				{
					correlation_id: "workspace_review",
					kind: "event",
					payload: { action: "reviewed" },
				},
			]);
			expect(output.review_duplicate).toMatchObject([
				{
					correlation_id: "workspace_review",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "duplicate" },
				},
			]);
			expect(output.review_unavailable).toMatchObject([
				{
					causation_id: "workspace_review_unavailable",
					correlation_id: "workspace_review_unavailable",
					kind: "command.receipt",
					payload: {
						error: { code: "workspace.unavailable", retryable: true },
						status: "rejected",
					},
					thread_id: "thread_workspace_protocol",
				},
			]);
			expect(output.rollback).toMatchObject([
				{
					correlation_id: "workspace_rollback",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "accepted" },
				},
				{
					correlation_id: "workspace_rollback",
					kind: "event",
					payload: { action: "rolled_back" },
				},
			]);
			expect(output.rollback_duplicate).toMatchObject([
				{
					correlation_id: "workspace_rollback",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "duplicate" },
				},
			]);
			expect(output.read_unavailable).toMatchObject([
				{
					correlation_id: "workspace_read_unavailable",
					kind: "protocol.error",
					payload: { code: "workspace.unavailable", retryable: true },
				},
			]);
			expect(output.list_unavailable).toMatchObject([
				{
					correlation_id: "workspace_list_unavailable",
					kind: "protocol.error",
					payload: { code: "projection.unavailable", retryable: true },
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects traffic before hello and versionless unsupported handshakes", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const before_hello_connection = yield* open_connection;

						yield* before_hello_connection.Receive(make_thread_create());

						const before_hello = yield* take_outbound(before_hello_connection, 1);
						const unsupported_connection = yield* open_connection;

						yield* unsupported_connection.Receive(
							make_hello("hello_unsupported", 0, [], [2]),
						);

						const unsupported = yield* take_outbound(unsupported_connection, 1);

						return { before_hello, unsupported };
					}),
				),
			);

			expect(output.before_hello).toMatchObject([
				{
					correlation_id: "command_1",
					kind: "protocol.error",
					payload: { code: "protocol.handshake_required", retryable: false },
				},
			]);
			expect(output.unsupported).toMatchObject([
				{
					correlation_id: "hello_unsupported",
					kind: "protocol.error",
					payload: { code: "protocol.unsupported_version", retryable: false },
				},
			]);
			expect(output.unsupported[0]).not.toHaveProperty("protocol_version");
		} finally {
			await runtime.dispose();
		}
	});

	it("preserves replay-from-origin for legacy and explicit resume hellos", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const legacy_connection = yield* open_connection;
						const fresh_shape = make_hello("hello_legacy");
						const { resume_mode: _resume_mode, ...legacy_payload } =
							fresh_shape.payload;
						const legacy = yield* negotiate(legacy_connection, {
							...fresh_shape,
							payload: legacy_payload,
						});
						const resume_connection = yield* open_connection;
						const resume = yield* negotiate(
							resume_connection,
							make_hello("hello_resume", 0, [], [1], "resume"),
						);

						return { legacy, resume };
					}),
				),
			);

			for (const handshake of [output.legacy, output.resume]) {
				expect(handshake.map((envelope) => envelope.kind)).toEqual([
					"welcome",
					"event",
					"replay.complete",
				]);
				expect(handshake[1]).toMatchObject({
					journal_sequence: 1,
					payload: { type: "guidance.canonical.updated" },
					stream_id: "settings:guidance",
				});
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("negotiates, returns a query snapshot, and validates acknowledgements", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;
						const handshake = yield* negotiate(connection);

						yield* connection.Receive(make_thread_create());

						const command = yield* take_outbound(connection, 2);
						const thread_id = created_thread_id(command);

						yield* connection.Receive(make_query());

						const query = yield* take_outbound(connection, 1);

						yield* connection.Receive(
							make_ack("ack_valid", 2, [
								{ sequence: 1, stream_id: "settings:guidance" },
								{ sequence: 1, stream_id: `thread:${thread_id}` },
							]),
						);
						yield* connection.Receive(
							make_ack("ack_duplicate_cursor", 2, [
								{ sequence: 1, stream_id: `thread:${thread_id}` },
								{ sequence: 1, stream_id: `thread:${thread_id}` },
							]),
						);
						yield* connection.Receive(
							make_ack("ack_projection_cursor", 2, [
								{
									sequence: 1,
									stream_id: "projection:thread.list:subscription_1",
								},
							]),
						);
						yield* connection.Receive(
							make_ack("ack_out_of_range", 3, [
								{ sequence: 2, stream_id: `thread:${thread_id}` },
							]),
						);

						const acknowledgement = yield* take_outbound(connection, 3);

						return { acknowledgement, command, handshake, query, thread_id };
					}),
				),
			);

			expect(output.handshake.map((envelope) => envelope.kind)).toEqual([
				"welcome",
				"replay.complete",
			]);
			expect(output.handshake[0]).toMatchObject({
				kind: "welcome",
				payload: {
					current_event_cursors: [{ sequence: 1, stream_id: "settings:guidance" }],
					journal_sequence: 1,
				},
			});
			expect(output.command).toMatchObject([
				{
					correlation_id: "command_1",
					kind: "thread.create.result",
					payload: { thread_id: output.thread_id, title: "Protocol integration" },
				},
				{
					correlation_id: "command_1",
					journal_sequence: 2,
					kind: "event",
				},
			]);
			expect(output.query).toMatchObject([
				{
					correlation_id: "query_1",
					kind: "thread.list.query.result",
					payload: {
						journal_sequence: 2,
						threads: [{ thread_id: output.thread_id, title: "Protocol integration" }],
					},
				},
			]);
			expect(
				output.acknowledgement.map((envelope) =>
					"correlation_id" in envelope ? envelope.correlation_id : undefined,
				),
			).toEqual(["ack_duplicate_cursor", "ack_projection_cursor", "ack_out_of_range"]);
			expect(output.acknowledgement).toMatchObject(
				Array.from({ length: 3 }, () => ({
					kind: "protocol.error",
					payload: { code: "protocol.invalid_ack", retryable: false },
				})),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("sends subscription snapshots and does not replay duplicate commands live", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive(make_subscribe());

						const subscription = yield* take_outbound(connection, 2);

						yield* connection.Receive(make_thread_create());

						const accepted = yield* take_outbound(connection, 3);
						const thread_id = created_thread_id(accepted);
						const duplicate_delivery = yield* take_until_outbound(
							connection,
							(envelope) => envelope.kind === "thread.list.query.result",
						).pipe(Effect.forkChild);

						yield* connection.Receive(make_thread_create());
						yield* connection.Receive(make_query("query_after_duplicate"));

						const after_duplicate = yield* Fiber.join(duplicate_delivery);

						return { accepted, after_duplicate, subscription, thread_id };
					}),
				),
			);

			expect(output.subscription).toMatchObject([
				{
					correlation_id: "subscribe_1",
					kind: "subscription.started",
					subscription_id: "subscription_1",
				},
				{
					kind: "thread.list.snapshot",
					payload: { threads: [] },
					sequence: 0,
					subscription_id: "subscription_1",
				},
			]);
			expect(output.accepted).toMatchObject([
				{
					kind: "thread.create.result",
					payload: { thread_id: output.thread_id },
				},
				{ kind: "event", journal_sequence: 2 },
				{
					kind: "thread.list.upsert",
					payload: {
						thread_id: output.thread_id,
						title: "Protocol integration",
					},
					sequence: 1,
				},
			]);
			expect(output.after_duplicate.map((envelope) => envelope.kind)).toEqual([
				"thread.create.result",
				"thread.list.query.result",
			]);
			expect(output.after_duplicate).toMatchObject([
				{
					correlation_id: "command_1",
					kind: "thread.create.result",
					payload: { thread_id: output.thread_id },
				},
				{
					correlation_id: "query_after_duplicate",
					kind: "thread.list.query.result",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("reconnects from a cursor and replays only missing events", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const replay = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const initial_connection = yield* open_connection;

						yield* negotiate(initial_connection);
						yield* initial_connection.Receive(make_thread_create());
						const initial = yield* take_outbound(initial_connection, 2);
						const first_thread_id = created_thread_id(initial);
						yield* initial_connection.Close;

						const producer_connection = yield* open_connection;

						yield* negotiate(producer_connection, make_hello("hello_producer"));
						yield* producer_connection.Receive(
							make_thread_create("command_2", "Missing event"),
						);
						const produced = yield* take_outbound(producer_connection, 2);
						const second_thread_id = created_thread_id(produced);

						const reconnecting_connection = yield* open_connection;

						yield* reconnecting_connection.Receive(
							make_hello(
								"hello_reconnect",
								2,
								[
									{ sequence: 1, stream_id: "settings:guidance" },
									{ sequence: 1, stream_id: `thread:${first_thread_id}` },
								],
								[1],
								"resume",
							),
						);

						return {
							replay: yield* take_outbound(reconnecting_connection, 3),
							second_thread_id,
						};
					}),
				),
			);

			expect(replay.replay.map((envelope) => envelope.kind)).toEqual([
				"welcome",
				"event",
				"replay.complete",
			]);
			expect(replay.replay[1]).toMatchObject({
				journal_sequence: 3,
				payload: { title: "Missing event", type: "thread.created" },
				thread_id: replay.second_thread_id,
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("broadcasts committed events to two live connections", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const observer_event = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const source_connection = yield* open_connection;
						const observer_connection = yield* open_connection;

						yield* negotiate(source_connection, make_hello("hello_source"));
						yield* negotiate(observer_connection, make_hello("hello_observer"));
						yield* source_connection.Receive(make_thread_create());
						const source_output = yield* take_outbound(source_connection, 2);

						return {
							event: yield* take_outbound(observer_connection, 1),
							thread_id: created_thread_id(source_output),
						};
					}),
				),
			);

			expect(observer_event.event).toMatchObject([
				{
					correlation_id: "command_1",
					journal_sequence: 2,
					kind: "event",
					thread_id: observer_event.thread_id,
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects malformed negotiated input and closes connections with their scope", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const malformed = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive({
							kind: "command",
							message_id: "malformed_command",
							origin: "frontend",
							protocol_version: 1,
							schema_version: 1,
							sent_at: "not-an-iso-date",
						});

						return yield* take_outbound(connection, 1);
					}),
				),
			);
			const closed_connection = await runtime.runPromise(Effect.scoped(open_connection));

			await runtime.runPromise(closed_connection.Closed);

			expect(malformed).toMatchObject([
				{
					kind: "protocol.error",
					payload: { code: "protocol.invalid_message", retryable: false },
					protocol_version: 1,
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("validates connection settings before constructing the server", async () => {
		const invalid_capacity = await Effect.runPromise(
			DecodeProtocolConnectionOptions({
				heartbeat_interval_ms: 10,
				heartbeat_timeout_ms: 20,
				outbound_capacity: 0,
			}).pipe(Effect.flip),
		);
		const invalid_timeout = await Effect.runPromise(
			DecodeProtocolConnectionOptions({
				heartbeat_interval_ms: 20,
				heartbeat_timeout_ms: 10,
				outbound_capacity: 1,
			}).pipe(Effect.flip),
		);

		expect(invalid_capacity._tag).toBe("ProtocolConfigurationError");
		expect(invalid_timeout._tag).toBe("ProtocolConfigurationError");
	});
});

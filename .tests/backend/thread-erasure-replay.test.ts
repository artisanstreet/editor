import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	HelloEnvelope,
	OutboundControlEnvelope,
	SubscribeEnvelope,
	ThreadCreateEnvelope,
	ThreadListQueryEnvelope,
} from "@artisan/protocol";
import {
	make_backend_runtime,
	ProjectionRebuildService,
	ProtocolServer,
	ThreadErasure,
	type ProtocolConnection,
} from "@artisan/backend";
import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	LegacyWorkspaceChangeProjections,
	NativeSubagentBindings,
	OrchestrationRuns,
	ThreadErasureClaims,
	WorkspaceChangeOperations,
	WorkspaceChanges,
	WorkspaceChangeSnapshots,
	WorkspaceMutationAuthorities,
} from "../../modules/backend/src/persistence/tables";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-thread-erasure-replay-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer(now: { value: string }) {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "backend_thread_erasure_replay_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.sync(() => now.value),
	});
}

function make_hello(
	message_id: string,
	last_journal_sequence = 0,
	event_cursors: HelloEnvelope["payload"]["event_cursors"] = [],
): HelloEnvelope {
	return {
		kind: "hello",
		message_id,
		origin: "frontend",
		payload: {
			event_cursors,
			last_journal_sequence,
			supported_protocol_versions: [1],
		},
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
	};
}

function make_create(message_id: string, title: string): ThreadCreateEnvelope {
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

function make_subscribe(subscription_id: string): SubscribeEnvelope {
	return {
		kind: "subscribe",
		message_id: `subscribe_${subscription_id}`,
		origin: "frontend",
		payload: { type: "thread.list" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
		subscription_id,
	};
}

function make_query(): ThreadListQueryEnvelope {
	return {
		kind: "thread.list.query",
		message_id: "query_after_repeat",
		origin: "frontend",
		payload: {},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
	};
}

function take_outbound(connection: ProtocolConnection, count: number) {
	return connection.Outbound.pipe(Stream.take(count), Stream.runCollect);
}

function take_through_replay_complete(connection: ProtocolConnection) {
	return connection.Outbound.pipe(
		Stream.takeUntil((envelope) => envelope.kind === "replay.complete"),
		Stream.runCollect,
	);
}

function event_envelopes(envelopes: ReadonlyArray<OutboundControlEnvelope>) {
	return envelopes.filter((envelope) => envelope.kind === "event");
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread erasure replay", () => {
	it("preserves interleaved cursors, removes once, and replays no erased content", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-10T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const database = yield* Database;
						const erasure = yield* ThreadErasure;
						const journal = yield* JournalStore;
						const server = yield* ProtocolServer;
						const threads = yield* ThreadReadModel;
						const live = yield* server.Open;

						yield* live.Receive(make_hello("hello_live"));
						yield* take_through_replay_complete(live);
						yield* live.Receive(make_subscribe("live_threads"));
						yield* take_outbound(live, 2);

						yield* live.Receive(make_create("create_erased", "Secret erased title"));
						const created_erased = yield* take_outbound(live, 3);
						const erased_create_result = created_erased.find(
							(envelope) => envelope.kind === "thread.create.result",
						);
						if (erased_create_result?.kind !== "thread.create.result") {
							return yield* Effect.die("Forge did not return the erased thread");
						}
						const erased_thread_id = erased_create_result.payload.thread_id;
						yield* database.client.insert(OrchestrationRuns).values({
							agent_id: "secret_native_root_agent",
							created_at: now.value,
							engine_id: "codex",
							native_resume_json: null,
							native_thread_id: "secret_native_root",
							run_id: "secret_native_root_run",
							status: "running",
							thread_id: erased_thread_id,
							updated_at: now.value,
							working_directory: "C:/workspace/erased",
						});
						yield* database.client.insert(NativeSubagentBindings).values({
							activity: "Private provider-native assignment",
							agent_id: "secret_native_child_agent",
							agent_native_thread_id: "secret_native_child",
							agent_path: "/root/private-child",
							assignment_id: "secret_native_assignment",
							binding_id: "secret_native_binding",
							created_at: now.value,
							engine_id: "codex",
							group_id: "secret_native_group",
							parent_native_thread_id: "secret_native_root",
							raw_origin_json: JSON.stringify({
								provider: "codex",
								reference: "private_provider_reference",
							}),
							root_run_id: "secret_native_root_run",
							run_id: "secret_native_child_run",
							state: "running",
							turn_id: "secret_native_turn",
							updated_at: now.value,
						});

						yield* live.Receive(make_create("create_kept", "Surviving thread"));
						const created_kept = yield* take_outbound(live, 3);
						const kept_create_result = created_kept.find(
							(envelope) => envelope.kind === "thread.create.result",
						);
						if (kept_create_result?.kind !== "thread.create.result") {
							return yield* Effect.die("Forge did not return the retained thread");
						}
						const kept_thread_id = kept_create_result.payload.thread_id;

						now.value = "2026-07-10T18:01:00.000Z";
						yield* journal.AppendEvent({
							agent_id: "secret_agent",
							causation_id: "secret_message_cause",
							correlation_id: "secret_message_correlation",
							payload: {
								message_id: "secret_message",
								reason: "no_active_run",
								text: "Private erased message body",
								type: "thread.message_queued",
								working_directory: "C:/workspace/erased",
							},
							raw_origin: {
								provider: "secret_provider",
								reference: "secret_raw_reference",
							},
							run_id: "secret_run",
							thread_id: erased_thread_id,
						});
						const erased_message = yield* take_outbound(live, 2);

						now.value = "2026-07-10T18:02:00.000Z";
						yield* journal.AppendEvent({
							causation_id: "kept_run_cause",
							correlation_id: "kept_run_correlation",
							payload: {
								state: "running",
								type: "run.lifecycle",
								working_directory: "C:/workspace/kept",
							},
							run_id: "kept_run",
							thread_id: kept_thread_id,
						});
						const kept_run = yield* take_outbound(live, 2);

						now.value = "2026-07-10T18:03:00.000Z";
						yield* journal.AppendEvent({
							agent_id: "secret_artifact_agent",
							causation_id: "secret_artifact_cause",
							correlation_id: "secret_artifact_correlation",
							payload: {
								artifact: {
									artifact_id: "secret_artifact",
									assignment_id: "secret_assignment",
									content: "Private erased artifact diff",
									created_at: now.value,
									group_id: "secret_group",
									kind: "diff",
									label: "Secret artifact label",
									run_id: "secret_artifact_run",
								},
								group_id: "secret_group",
								type: "artifact.recorded",
							},
							raw_origin: {
								provider: "secret_artifact_provider",
								reference: "secret_artifact_reference",
							},
							run_id: "secret_artifact_run",
							thread_id: erased_thread_id,
						});
						const erased_artifact = yield* take_outbound(live, 2);
						const content_identity = JSON.stringify({
							algorithm: "sha256",
							byte_count: 1,
							content_hash: "a".repeat(64),
						});

						yield* database.client.insert(WorkspaceChangeOperations).values({
							action: "replace",
							agent_id: "secret_workspace_agent",
							change_id: "secret_workspace_change",
							created_at: now.value,
							expected_identity_json: content_identity,
							lifecycle: "committed",
							message_id: "secret_workspace_command",
							path: "secret.ts",
							request_fingerprint: "b".repeat(64),
							result_identity_json: content_identity,
							run_id: "secret_workspace_run",
							sent_at: now.value,
							thread_id: erased_thread_id,
							updated_at: now.value,
							workspace_id: "secret_workspace",
						});
						yield* database.client.insert(WorkspaceMutationAuthorities).values({
							agent_id: "secret_workspace_agent",
							approval: null,
							assignment_id: null,
							authority_kind: "base_run",
							change_id: "secret_workspace_change",
							created_at: now.value,
							group_id: null,
							message_id: "secret_workspace_command",
							run_id: "secret_workspace_run",
							scope_kind: null,
							scope_value: null,
							thread_id: erased_thread_id,
							working_directory: "C:/workspace/erased",
							workspace_id: "secret_workspace",
						});
						yield* database.client.insert(WorkspaceChanges).values({
							after_identity_json: content_identity,
							agent_id: "secret_workspace_agent",
							before_identity_json: content_identity,
							change_id: "secret_workspace_change",
							created_at: now.value,
							path: "secret.ts",
							review_state: "needs_review",
							rollback_state: "available",
							run_id: "secret_workspace_run",
							source_command_id: "secret_workspace_command",
							thread_id: erased_thread_id,
							updated_at: now.value,
							version: 1,
							workspace_id: "secret_workspace",
						});
						yield* database.client.insert(LegacyWorkspaceChangeProjections).values({
							change_id: "secret_workspace_change",
							source_command_id: "secret_workspace_command",
							thread_id: erased_thread_id,
						});
						yield* database.client.insert(WorkspaceChangeSnapshots).values({
							byte_count: 22,
							change_id: "secret_workspace_snapshot",
							content: Buffer.from("private rollback bytes"),
							content_hash: "c".repeat(64),
							created_at: now.value,
							state: "available",
							thread_id: erased_thread_id,
							updated_at: now.value,
						});

						yield* database.client.insert(ThreadErasureClaims).values({
							claimed_at: "2026-07-10T18:04:00.000Z",
							thread_id: erased_thread_id,
						});
						const erased = yield* erasure.ResumeClaimed("2026-07-10T18:04:00.000Z");
						const rebuild = yield* ProjectionRebuildService;
						const rebuilt = yield* rebuild.Rebuild();
						const erased_delivery = yield* take_outbound(live, 2);
						const repeated = yield* erasure.ResumeClaimed("2026-07-10T18:04:00.000Z");

						yield* live.Receive(make_query());
						const after_repeat = yield* take_outbound(live, 1);

						now.value = "2026-07-10T18:05:00.000Z";
						yield* journal.AppendEvent({
							causation_id: "kept_later_cause",
							correlation_id: "kept_later_correlation",
							payload: {
								message_id: "kept_later_message",
								reason: "unsupported",
								text: "Surviving later event",
								type: "thread.message_queued",
								working_directory: "C:/workspace/kept",
							},
							thread_id: kept_thread_id,
						});
						const kept_later = yield* take_outbound(live, 2);
						yield* live.Close;

						const reconnect = yield* server.Open;

						yield* reconnect.Receive(
							make_hello("hello_reconnect", 3, [
								{ sequence: 1, stream_id: "settings:guidance" },
								{ sequence: 1, stream_id: `thread:${erased_thread_id}` },
								{ sequence: 1, stream_id: `thread:${kept_thread_id}` },
							]),
						);
						const reconnect_replay = yield* take_through_replay_complete(reconnect);

						yield* reconnect.Receive(make_subscribe("fresh_threads"));
						const fresh_subscription = yield* take_outbound(reconnect, 2);

						return {
							after_repeat,
							created_erased,
							created_kept,
							current_cursors: yield* journal.ReadCurrentCursors(),
							erased,
							erased_thread_id,
							erased_artifact,
							erased_delivery,
							erased_message,
							fresh_subscription,
							full_replay: yield* journal.ReadReplay({ after_journal_sequence: 0 }),
							journal_commands: yield* database.client.select().from(JournalCommands),
							journal_events: yield* database.client.select().from(JournalEvents),
							legacy_workspace_change_projections: yield* database.client
								.select()
								.from(LegacyWorkspaceChangeProjections),
							native_subagent_bindings: yield* database.client
								.select()
								.from(NativeSubagentBindings),
							kept_later,
							kept_thread_id,
							kept_run,
							reconnect_replay,
							repeated,
							rebuilt,
							streams: yield* database.client.select().from(EventStreams),
							thread_snapshot: yield* threads.Snapshot(),
							workspace_change_operations: yield* database.client
								.select()
								.from(WorkspaceChangeOperations),
							workspace_changes: yield* database.client
								.select()
								.from(WorkspaceChanges),
							workspace_change_snapshots: yield* database.client
								.select()
								.from(WorkspaceChangeSnapshots),
							workspace_mutation_authorities: yield* database.client
								.select()
								.from(WorkspaceMutationAuthorities),
						};
					}),
				),
			);

			const live_envelopes = [
				...result.created_erased,
				...result.created_kept,
				...result.erased_message,
				...result.kept_run,
				...result.erased_artifact,
				...result.erased_delivery,
				...result.kept_later,
			];
			const projection_updates = live_envelopes.filter(
				(envelope) =>
					envelope.kind === "thread.list.upsert" ||
					envelope.kind === "thread.list.remove",
			);
			const removals = projection_updates.filter(
				(envelope) => envelope.kind === "thread.list.remove",
			);
			const replay_events = event_envelopes(result.reconnect_replay);
			const erased_stream = result.full_replay.filter(
				(event) => event.thread_id === result.erased_thread_id,
			);
			const kept_stream = result.full_replay.filter(
				(event) => event.thread_id === result.kept_thread_id,
			);
			const serialized_replay = JSON.stringify(result.full_replay);
			const serialized_reconnect_replay = JSON.stringify(result.reconnect_replay);

			expect(result.erased).toEqual([result.erased_thread_id]);
			expect(result.repeated).toEqual([]);
			expect(result.rebuilt.equivalent).toBe(true);
			expect(result.after_repeat).toMatchObject([{ kind: "thread.list.query.result" }]);
			expect(projection_updates.map((envelope) => envelope.sequence)).toEqual([
				1, 2, 3, 4, 5, 6, 7,
			]);
			expect(removals).toMatchObject([
				{
					journal_sequence: 7,
					kind: "thread.list.remove",
					payload: { thread_id: result.erased_thread_id },
					sequence: 6,
				},
			]);
			expect(result.erased_delivery.map((envelope) => envelope.kind)).toEqual([
				"event",
				"thread.list.remove",
			]);
			expect(result.erased_delivery[0]).toMatchObject({
				journal_sequence: 7,
				payload: { type: "thread.erased" },
			});

			expect(result.full_replay.map((event) => event.journal_sequence)).toEqual([
				1, 2, 3, 4, 5, 6, 7, 8,
			]);
			expect(result.full_replay[0]).toMatchObject({
				journal_sequence: 1,
				payload: {
					byte_count: 0,
					content_hash:
						"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
					type: "guidance.canonical.updated",
				},
				stream_id: "settings:guidance",
				thread_id: "settings/guidance",
			});
			expect(erased_stream.map((event) => event.sequence)).toEqual([1, 2, 3, 4]);
			expect(erased_stream.map((event) => event.payload.type)).toEqual([
				"thread.content_erased",
				"thread.content_erased",
				"thread.content_erased",
				"thread.erased",
			]);
			expect(kept_stream.map((event) => event.sequence)).toEqual([1, 2, 3]);
			expect(kept_stream.map((event) => event.payload.type)).toEqual([
				"thread.created",
				"run.lifecycle",
				"thread.message_queued",
			]);
			expect(serialized_replay).not.toContain("Secret erased title");
			expect(serialized_replay).not.toContain("Private erased message body");
			expect(serialized_replay).not.toContain("Private erased artifact diff");
			expect(serialized_replay).not.toContain("secret_raw_reference");
			expect(serialized_replay).not.toContain("secret_agent");
			expect(serialized_reconnect_replay).not.toContain("Secret erased title");
			expect(serialized_reconnect_replay).not.toContain("Private erased message body");
			expect(serialized_reconnect_replay).not.toContain("Private erased artifact diff");
			expect(serialized_reconnect_replay).not.toContain("secret_raw_reference");
			expect(serialized_reconnect_replay).not.toContain("secret_agent");

			expect(replay_events.map((event) => event.journal_sequence)).toEqual([4, 5, 6, 7, 8]);
			expect(
				replay_events
					.filter((event) => event.thread_id === result.erased_thread_id)
					.map((event) => event.sequence),
			).toEqual([2, 3, 4]);
			expect(
				replay_events
					.filter((event) => event.thread_id === result.kept_thread_id)
					.map((event) => event.sequence),
			).toEqual([2, 3]);
			expect(result.reconnect_replay.at(-1)).toMatchObject({
				kind: "replay.complete",
				payload: {
					current_event_cursors: [
						{ sequence: 1, stream_id: "settings:guidance" },
						{ sequence: 4, stream_id: `thread:${result.erased_thread_id}` },
						{ sequence: 3, stream_id: `thread:${result.kept_thread_id}` },
					],
					journal_sequence: 8,
				},
			});

			expect(result.current_cursors).toEqual([
				{ sequence: 1, stream_id: "settings:guidance" },
				{ sequence: 4, stream_id: `thread:${result.erased_thread_id}` },
				{ sequence: 3, stream_id: `thread:${result.kept_thread_id}` },
			]);
			expect(result.streams).toEqual([
				{ last_sequence: 1, stream_id: "settings:guidance" },
				{ last_sequence: 4, stream_id: `thread:${result.erased_thread_id}` },
				{ last_sequence: 3, stream_id: `thread:${result.kept_thread_id}` },
			]);
			expect(result.thread_snapshot.threads).toMatchObject([
				{ thread_id: result.kept_thread_id, title: "Surviving later event" },
			]);
			expect(result.fresh_subscription[1]).toMatchObject({
				kind: "thread.list.snapshot",
				payload: {
					threads: [{ thread_id: result.kept_thread_id, title: "Surviving later event" }],
				},
			});
			expect(
				result.journal_commands.filter(
					(command) => command.thread_id === result.erased_thread_id,
				),
			).toEqual([]);
			expect(
				result.journal_events.filter(
					(event) => event.thread_id === result.erased_thread_id,
				),
			).toHaveLength(4);
			expect(result.workspace_change_operations).toEqual([]);
			expect(result.workspace_changes).toEqual([]);
			expect(result.workspace_change_snapshots).toEqual([]);
			expect(result.workspace_mutation_authorities).toEqual([]);
			expect(result.legacy_workspace_change_projections).toEqual([]);
			expect(result.native_subagent_bindings).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});
});

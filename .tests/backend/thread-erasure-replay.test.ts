import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Schema, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	CommandEnvelope as CommandEnvelopeSchema,
	type CommandEnvelope,
	HelloEnvelope,
	OutboundControlEnvelope,
	SubscribeEnvelope,
	ThreadListQueryEnvelope,
} from "@artisan/protocol";
import {
	make_backend_runtime,
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
	PreviewTargetProbeClaims,
	ThreadErasureClaims,
	WorkspaceChangeOperations,
	WorkspaceChanges,
	WorkspaceChangeSnapshots,
	WorkspaceMutationAuthorities,
} from "../../modules/backend/src/persistence/schema";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const EncodeCommandJson = Schema.encodeEffect(Schema.fromJsonString(CommandEnvelopeSchema), {
	onExcessProperty: "error",
});

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

function make_create(message_id: string, thread_id: string, title: string): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: { title, type: "thread.create" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
		thread_id,
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

						yield* live.Receive(
							make_create("create_erased", "thread_erased", "Secret erased title"),
						);
						const created_erased = yield* take_outbound(live, 3);
						yield* live.Receive(
							make_create("create_kept", "thread_kept", "Surviving thread"),
						);
						const created_kept = yield* take_outbound(live, 3);

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
							thread_id: "thread_erased",
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
							thread_id: "thread_kept",
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
							thread_id: "thread_erased",
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
							thread_id: "thread_erased",
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
							thread_id: "thread_erased",
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
							thread_id: "thread_erased",
							updated_at: now.value,
							version: 1,
							workspace_id: "secret_workspace",
						});
						yield* database.client.insert(WorkspaceChangeSnapshots).values({
							byte_count: 22,
							change_id: "secret_workspace_snapshot",
							content: Buffer.from("private rollback bytes"),
							content_hash: "c".repeat(64),
							created_at: now.value,
							state: "available",
							thread_id: "thread_erased",
							updated_at: now.value,
						});
						const preview_probe_command = {
							kind: "command",
							message_id: "secret_preview_probe",
							origin: "frontend",
							payload: {
								project_id: "secret_project",
								target_id: "secret_preview",
								type: "preview.target.probe",
								workspace_id: "secret_workspace",
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at: now.value,
							thread_id: "thread_erased",
						} satisfies CommandEnvelope;
						const preview_probe_command_json =
							yield* EncodeCommandJson(preview_probe_command);

						yield* database.client.insert(PreviewTargetProbeClaims).values({
							claim_token: "claim_secret_preview_probe",
							command_json: preview_probe_command_json,
							created_at: now.value,
							lease_expires_at: "2026-07-10T18:10:00.000Z",
							message_id: preview_probe_command.message_id,
							owner_instance_id: "backend_secret_preview_owner",
							project_id: preview_probe_command.payload.project_id,
							target_id: preview_probe_command.payload.target_id,
							target_generation_id: "preview_target_secret",
							thread_id: preview_probe_command.thread_id,
							updated_at: now.value,
							workspace_id: preview_probe_command.payload.workspace_id,
						});

						yield* database.client.insert(ThreadErasureClaims).values({
							claimed_at: "2026-07-10T18:04:00.000Z",
							thread_id: "thread_erased",
						});
						const erased = yield* erasure.ResumeClaimed("2026-07-10T18:04:00.000Z");
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
							thread_id: "thread_kept",
						});
						const kept_later = yield* take_outbound(live, 2);
						yield* live.Close;

						const reconnect = yield* server.Open;

						yield* reconnect.Receive(
							make_hello("hello_reconnect", 3, [
								{ sequence: 1, stream_id: "settings:guidance" },
								{ sequence: 1, stream_id: "thread:thread_erased" },
								{ sequence: 1, stream_id: "thread:thread_kept" },
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
							erased_artifact,
							erased_delivery,
							erased_message,
							fresh_subscription,
							full_replay: yield* journal.ReadReplay({ after_journal_sequence: 0 }),
							journal_commands: yield* database.client.select().from(JournalCommands),
							journal_events: yield* database.client.select().from(JournalEvents),
							kept_later,
							kept_run,
							preview_probe_claims: yield* database.client
								.select()
								.from(PreviewTargetProbeClaims),
							reconnect_replay,
							repeated,
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
				(event) => event.thread_id === "thread_erased",
			);
			const kept_stream = result.full_replay.filter(
				(event) => event.thread_id === "thread_kept",
			);
			const serialized_replay = JSON.stringify(result.full_replay);
			const serialized_reconnect_replay = JSON.stringify(result.reconnect_replay);

			expect(result.erased).toEqual(["thread_erased"]);
			expect(result.repeated).toEqual([]);
			expect(result.after_repeat).toMatchObject([{ kind: "thread.list.query.result" }]);
			expect(projection_updates.map((envelope) => envelope.sequence)).toEqual([
				1, 2, 3, 4, 5, 6, 7,
			]);
			expect(removals).toMatchObject([
				{
					journal_sequence: 7,
					kind: "thread.list.remove",
					payload: { thread_id: "thread_erased" },
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
					.filter((event) => event.thread_id === "thread_erased")
					.map((event) => event.sequence),
			).toEqual([2, 3, 4]);
			expect(
				replay_events
					.filter((event) => event.thread_id === "thread_kept")
					.map((event) => event.sequence),
			).toEqual([2, 3]);
			expect(result.reconnect_replay.at(-1)).toMatchObject({
				kind: "replay.complete",
				payload: {
					current_event_cursors: [
						{ sequence: 1, stream_id: "settings:guidance" },
						{ sequence: 4, stream_id: "thread:thread_erased" },
						{ sequence: 3, stream_id: "thread:thread_kept" },
					],
					journal_sequence: 8,
				},
			});

			expect(result.current_cursors).toEqual([
				{ sequence: 1, stream_id: "settings:guidance" },
				{ sequence: 4, stream_id: "thread:thread_erased" },
				{ sequence: 3, stream_id: "thread:thread_kept" },
			]);
			expect(result.streams).toEqual([
				{ last_sequence: 1, stream_id: "settings:guidance" },
				{ last_sequence: 4, stream_id: "thread:thread_erased" },
				{ last_sequence: 3, stream_id: "thread:thread_kept" },
			]);
			expect(result.thread_snapshot.threads).toMatchObject([
				{ thread_id: "thread_kept", title: "Surviving thread" },
			]);
			expect(result.fresh_subscription[1]).toMatchObject({
				kind: "thread.list.snapshot",
				payload: { threads: [{ thread_id: "thread_kept" }] },
			});
			expect(
				result.journal_commands.filter((command) => command.thread_id === "thread_erased"),
			).toEqual([]);
			expect(
				result.journal_events.filter((event) => event.thread_id === "thread_erased"),
			).toHaveLength(4);
			expect(result.workspace_change_operations).toEqual([]);
			expect(result.workspace_changes).toEqual([]);
			expect(result.workspace_change_snapshots).toEqual([]);
			expect(result.workspace_mutation_authorities).toEqual([]);
			expect(result.preview_probe_claims).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});
});

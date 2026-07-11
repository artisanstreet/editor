import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	CommandEnvelope,
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
	ThreadErasureClaims,
} from "../../modules/backend/src/persistence/schema";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

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
						yield* take_outbound(live, 2);
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
							payload: { state: "running", type: "run.lifecycle" },
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
							},
							thread_id: "thread_kept",
						});
						const kept_later = yield* take_outbound(live, 2);
						yield* live.Close;

						const reconnect = yield* server.Open;

						yield* reconnect.Receive(
							make_hello("hello_reconnect", 2, [
								{ sequence: 1, stream_id: "thread:thread_erased" },
								{ sequence: 1, stream_id: "thread:thread_kept" },
							]),
						);
						const reconnect_replay = yield* take_outbound(reconnect, 7);

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
							reconnect_replay,
							repeated,
							streams: yield* database.client.select().from(EventStreams),
							thread_snapshot: yield* threads.Snapshot(),
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

			expect(result.erased).toEqual(["thread_erased"]);
			expect(result.repeated).toEqual([]);
			expect(result.after_repeat).toMatchObject([{ kind: "thread.list.query.result" }]);
			expect(projection_updates.map((envelope) => envelope.sequence)).toEqual([
				1, 2, 3, 4, 5, 6, 7,
			]);
			expect(removals).toMatchObject([
				{
					journal_sequence: 6,
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
				journal_sequence: 6,
				payload: { type: "thread.erased" },
			});

			expect(result.full_replay.map((event) => event.journal_sequence)).toEqual([
				1, 2, 3, 4, 5, 6, 7,
			]);
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

			expect(replay_events.map((event) => event.journal_sequence)).toEqual([3, 4, 5, 6, 7]);
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
						{ sequence: 4, stream_id: "thread:thread_erased" },
						{ sequence: 3, stream_id: "thread:thread_kept" },
					],
					journal_sequence: 7,
				},
			});

			expect(result.current_cursors).toEqual([
				{ sequence: 4, stream_id: "thread:thread_erased" },
				{ sequence: 3, stream_id: "thread:thread_kept" },
			]);
			expect(result.streams).toEqual([
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
		} finally {
			await runtime.dispose();
		}
	});
});

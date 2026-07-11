import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Fiber, Layer, Ref, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { EventEnvelope } from "@artisan/protocol";
import { make_backend_runtime, ProtocolServer, ThreadErasure } from "@artisan/backend";
import type { ThreadListUpdate } from "@artisan/transport";
import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { ThreadErasureClaims } from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

import {
	make_transport_test_harness_with_protocol_server,
	wait_for,
} from "./message-channel-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-client-protocol-server-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer(now: { value: string }) {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "artisan_client_protocol_server_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.sync(() => now.value),
	});
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("ArtisanClient with the backend ProtocolServer", () => {
	it("replays an interleaved journal through real MessagePorts without resurrecting erased content", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-10T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(now),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const erasure = await runtime.runPromise(ThreadErasure);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
			client: { reconnect_delay_ms: 5 },
		});
		let replay_harness:
			| Awaited<ReturnType<typeof make_transport_test_harness_with_protocol_server>>
			| undefined;

		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const thread_updates = yield* harness.client.SubscribeThreadList;
						const erasure_delivered = yield* Deferred.make<void>();
						const initial_snapshot = yield* Deferred.make<void>();
						const reconnect_snapshot = yield* Deferred.make<void>();
						const removal_delivered = yield* Deferred.make<void>();
						const replayed_kept_event = yield* Deferred.make<void>();
						const updates = yield* Ref.make<ReadonlyArray<ThreadListUpdate>>([]);
						const events = yield* Ref.make<ReadonlyArray<EventEnvelope>>([]);
						let snapshot_count = 0;

						yield* thread_updates.pipe(
							Stream.tap((update) =>
								Effect.gen(function* () {
									yield* Ref.update(updates, (current) => [...current, update]);

									if (update.type === "snapshot") {
										snapshot_count += 1;
										yield* Deferred.succeed(initial_snapshot, undefined).pipe(
											Effect.asVoid,
										);

										if (snapshot_count >= 2) {
											yield* Deferred.succeed(
												reconnect_snapshot,
												undefined,
											).pipe(Effect.asVoid);
										}
									}

									if (
										update.type === "remove" &&
										update.thread_id === "thread_erased"
									) {
										yield* Deferred.succeed(removal_delivered, undefined).pipe(
											Effect.asVoid,
										);
									}
								}),
							),
							Stream.runDrain,
							Effect.forkScoped,
						);
						yield* harness.client.Events.pipe(
							Stream.tap((event) =>
								Effect.gen(function* () {
									yield* Ref.update(events, (current) => [...current, event]);

									if (event.journal_sequence === 6) {
										yield* Deferred.succeed(erasure_delivered, undefined).pipe(
											Effect.asVoid,
										);
									}

									if (event.journal_sequence === 7) {
										yield* Deferred.succeed(
											replayed_kept_event,
											undefined,
										).pipe(Effect.asVoid);
									}
								}),
							),
							Stream.runDrain,
							Effect.forkScoped,
						);

						yield* Deferred.await(initial_snapshot);

						yield* harness.client.Command({
							command_id: "create_erased",
							payload: { title: "Secret erased title", type: "thread.create" },
							thread_id: "thread_erased",
						});
						yield* harness.client.Command({
							command_id: "create_kept",
							payload: { title: "Surviving thread", type: "thread.create" },
							thread_id: "thread_kept",
						});

						now.value = "2026-07-10T18:01:00.000Z";
						yield* journal.AppendEvent({
							causation_id: "secret_message_cause",
							correlation_id: "secret_message_correlation",
							payload: {
								message_id: "secret_message",
								reason: "no_active_run",
								text: "Private erased message body",
								type: "thread.message_queued",
							},
							thread_id: "thread_erased",
						});

						now.value = "2026-07-10T18:02:00.000Z";
						yield* journal.AppendEvent({
							causation_id: "kept_run_cause",
							correlation_id: "kept_run_correlation",
							payload: { state: "running", type: "run.lifecycle" },
							run_id: "kept_run",
							thread_id: "thread_kept",
						});

						now.value = "2026-07-10T18:03:00.000Z";
						yield* journal.AppendEvent({
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
							thread_id: "thread_erased",
						});

						yield* database.client.insert(ThreadErasureClaims).values({
							claimed_at: "2026-07-10T18:04:00.000Z",
							thread_id: "thread_erased",
						});
						yield* erasure.ResumeClaimed("2026-07-10T18:04:00.000Z");
						yield* Deferred.await(erasure_delivered);
						yield* Deferred.await(removal_delivered);

						yield* Effect.sync(harness.close_current_connection);
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
						yield* Effect.promise(() =>
							wait_for(() => harness.connector_snapshot().connections >= 2),
						);
						yield* Deferred.await(replayed_kept_event);
						yield* Deferred.await(reconnect_snapshot);

						return {
							cursors: yield* harness.client.Cursors,
							events: yield* Ref.get(events),
							updates: yield* Ref.get(updates),
						};
					}),
				),
			);
			replay_harness = await make_transport_test_harness_with_protocol_server(
				protocol_server,
				{
					client: { reconnect_delay_ms: 5 },
				},
			);
			const current_replay_harness = replay_harness;
			const replayed_events = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const events = current_replay_harness.client.Events;
						const replay_fiber = yield* events.pipe(
							Stream.take(7),
							Stream.runCollect,
							Effect.forkScoped,
						);

						yield* Effect.promise(() =>
							wait_for(
								() => current_replay_harness.connector_snapshot().connections >= 1,
							),
						);
						yield* current_replay_harness.client.ListThreads;

						return [...(yield* Fiber.join(replay_fiber))];
					}),
				),
			);
			const removal_index = output.updates.findIndex(
				(update) => update.type === "remove" && update.thread_id === "thread_erased",
			);
			const serialized_replay = JSON.stringify(replayed_events);

			expect(output.events.map((event) => event.journal_sequence)).toEqual([
				1, 2, 3, 4, 5, 6, 7,
			]);
			expect(output.events.map((event) => event.sequence)).toEqual([1, 1, 2, 2, 3, 4, 3]);
			expect(output.events.map((event) => event.thread_id)).toEqual([
				"thread_erased",
				"thread_kept",
				"thread_erased",
				"thread_kept",
				"thread_erased",
				"thread_erased",
				"thread_kept",
			]);
			expect(output.updates[0]).toMatchObject({ type: "snapshot", threads: [] });
			expect(output.updates).toContainEqual(
				expect.objectContaining({
					thread: expect.objectContaining({
						activity_version: 1,
						thread_id: "thread_kept",
					}),
					type: "upsert",
				}),
			);
			expect(removal_index).toBeGreaterThan(0);
			expect(output.updates[removal_index]).toMatchObject({
				journal_sequence: 6,
				thread_id: "thread_erased",
				type: "remove",
			});
			expect(output.updates.slice(removal_index + 1)).not.toContainEqual(
				expect.objectContaining({
					thread: expect.objectContaining({ thread_id: "thread_erased" }),
				}),
			);
			expect(output.updates.slice(removal_index + 1)).not.toContainEqual(
				expect.objectContaining({
					threads: expect.arrayContaining([
						expect.objectContaining({ thread_id: "thread_erased" }),
					]),
				}),
			);
			expect(output.cursors).toEqual({
				event_cursors: [
					{ sequence: 4, stream_id: "thread:thread_erased" },
					{ sequence: 3, stream_id: "thread:thread_kept" },
				],
				last_journal_sequence: 7,
			});
			expect(serialized_replay).not.toContain("Secret erased title");
			expect(serialized_replay).not.toContain("Private erased message body");
			expect(serialized_replay).not.toContain("Private erased artifact diff");
		} finally {
			await replay_harness?.dispose();
			await harness.dispose();
			await runtime.dispose();
		}
	});
});

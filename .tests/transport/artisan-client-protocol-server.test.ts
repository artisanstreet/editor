import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Fiber, Layer, Ref, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { EventEnvelope } from "@artisan/protocol";
import {
	make_backend_runtime,
	make_guidance_provider_registry_layer,
	ProtocolServer,
	ThreadErasure,
} from "@artisan/backend";
import type { ThreadListUpdate } from "@artisan/transport";
import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import {
	GlobalGuidanceCanonical,
	GlobalGuidanceProviderSync,
	JournalCommands,
	JournalEvents,
	ThreadErasureClaims,
} from "../../modules/backend/src/persistence/schema";
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
	it("queries and updates global guidance through real MessagePorts", async () => {
		const database_path = await make_database_path();
		const canonical_path = join(dirname(database_path), "guidance", "GLOBAL.md");
		const runtime = make_backend_runtime({
			database_path,
			guidance: { canonical_path },
			migrations_path,
			runtime_metadata: make_metadata_layer({ value: "2026-07-10T18:00:00.000Z" }),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);

		try {
			const initial = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const receipt = await Effect.runPromise(
				harness.client.UpdateGlobalGuidance({
					command_id: "real_guidance_update_1",
					content: "Real transport guidance\n",
				}),
			);
			const updated = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const canonical_file = await readFile(canonical_path, "utf8");
			const metadata_rows = await Effect.runPromise(
				Effect.gen(function* () {
					const canonical = yield* database.client.select().from(GlobalGuidanceCanonical);
					const providers = yield* database.client
						.select()
						.from(GlobalGuidanceProviderSync);
					const commands = yield* database.client.select().from(JournalCommands);
					const events = yield* database.client.select().from(JournalEvents);

					return { canonical, commands, events, providers };
				}),
			);
			const query_event_count = metadata_rows.events.length;
			const queried = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const after_query_events = await Effect.runPromise(
				database.client.select().from(JournalEvents),
			);
			const changed_content = await Effect.runPromise(
				harness.client
					.UpdateGlobalGuidance({
						command_id: "real_guidance_update_1",
						content: "Changed real transport guidance\n",
					})
					.pipe(Effect.flip),
			);
			const after_conflict = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const after_conflict_file = await readFile(canonical_path, "utf8");
			const after_conflict_rows = await Effect.runPromise(
				Effect.gen(function* () {
					return {
						canonical: yield* database.client.select().from(GlobalGuidanceCanonical),
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
					};
				}),
			);
			const journal_rows = JSON.stringify({
				commands: metadata_rows.commands,
				events: metadata_rows.events,
			});

			expect(initial.content).toBe("");
			expect(receipt).toMatchObject({
				command_id: "real_guidance_update_1",
				status: "accepted",
			});
			expect(updated.content).toBe("Real transport guidance\n");
			expect(canonical_file).toBe("Real transport guidance\n");
			expect(metadata_rows.canonical).toHaveLength(1);
			expect(metadata_rows.canonical[0]).toMatchObject({
				byte_count: Buffer.byteLength("Real transport guidance\n"),
				content_hash: updated.metadata.canonical.content_hash,
				status: "ready",
			});
			expect(metadata_rows.providers).toEqual([]);
			expect(metadata_rows.commands).toHaveLength(2);
			expect(metadata_rows.events).toHaveLength(2);
			expect(metadata_rows.events.map((event) => event.stream_id)).toEqual([
				"settings:guidance",
				"settings:guidance",
			]);
			expect(journal_rows).toContain(updated.metadata.canonical.content_hash);
			expect(journal_rows).not.toContain("Real transport guidance");
			expect(queried).toEqual(updated);
			expect(after_query_events).toHaveLength(query_event_count);
			expect(changed_content).toMatchObject({
				code: "protocol",
				protocol_code: "command.id_conflict",
				retryable: false,
			});
			expect(after_conflict).toEqual(updated);
			expect(after_conflict_file).toBe(canonical_file);
			expect(after_conflict_rows.commands).toHaveLength(metadata_rows.commands.length);
			expect(after_conflict_rows.events).toHaveLength(metadata_rows.events.length);
			expect(after_conflict_rows.canonical).toEqual(metadata_rows.canonical);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("rejects a stale first-run source and accepts an exact provider candidate", async () => {
		const database_path = await make_database_path();
		const root = dirname(database_path);
		const canonical_path = join(root, "guidance", "GLOBAL.md");
		const codex_content = "Codex candidate guidance\n";
		const claude_content = "Claude candidate guidance\n";
		const candidate = (provider: "claude" | "codex", content: string, path: string) => ({
			Discover: Effect.succeed({
				_tag: "Present" as const,
				content,
				hash: createHash("sha256").update(content).digest("hex"),
				modified_at: "2026-07-10T17:00:00.000Z",
				path,
			}),
			mode: "native_file" as const,
			provider,
		});
		const runtime = make_backend_runtime({
			database_path,
			guidance: { canonical_path },
			guidance_provider_registry: make_guidance_provider_registry_layer([
				candidate("codex", codex_content, join(root, "providers", "AGENTS.md")),
				candidate("claude", claude_content, join(root, "providers", "CLAUDE.md")),
			]),
			migrations_path,
			runtime_metadata: make_metadata_layer({ value: "2026-07-10T18:00:00.000Z" }),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);

		try {
			const initial = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const before_conflict = await Effect.runPromise(
				Effect.all({
					canonical: database.client.select().from(GlobalGuidanceCanonical),
					commands: database.client.select().from(JournalCommands),
					events: database.client.select().from(JournalEvents),
					providers: database.client.select().from(GlobalGuidanceProviderSync),
				}),
			);
			const stale = await Effect.runPromise(
				harness.client
					.SelectGlobalGuidance({
						command_id: "stale_guidance_selection",
						content_hash: "f".repeat(64),
						provider: "codex",
					})
					.pipe(Effect.flip),
			);
			const after_conflict = await Effect.runPromise(
				Effect.all({
					canonical: database.client.select().from(GlobalGuidanceCanonical),
					commands: database.client.select().from(JournalCommands),
					events: database.client.select().from(JournalEvents),
					providers: database.client.select().from(GlobalGuidanceProviderSync),
				}),
			);
			const codex_candidate = initial.candidates.find(
				({ provider }) => provider === "codex",
			)!;
			const accepted = await Effect.runPromise(
				harness.client.SelectGlobalGuidance({
					command_id: "valid_guidance_selection",
					content_hash: codex_candidate.content_hash,
					provider: "codex",
				}),
			);
			const selected = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const persisted = await Effect.runPromise(
				Effect.all({
					canonical: database.client.select().from(GlobalGuidanceCanonical),
					commands: database.client.select().from(JournalCommands),
					events: database.client.select().from(JournalEvents),
					providers: database.client.select().from(GlobalGuidanceProviderSync),
				}),
			);
			const persisted_json = JSON.stringify(persisted);

			expect(initial.metadata.canonical.status).toBe("selection_required");
			expect(initial.candidates).toHaveLength(2);
			expect(stale).toMatchObject({
				code: "protocol",
				protocol_code: "guidance.conflict",
				retryable: false,
			});
			expect(after_conflict).toEqual(before_conflict);
			expect(accepted.status).toBe("accepted");
			expect(selected.content).toBe(codex_content);
			expect(await readFile(canonical_path, "utf8")).toBe(codex_content);
			expect(persisted_json).not.toContain(codex_content.trim());
			expect(persisted_json).not.toContain(claude_content.trim());
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

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
						const guidance_delivered = yield* Deferred.make<void>();
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

									if (event.payload.type === "guidance.canonical.updated") {
										yield* Deferred.succeed(guidance_delivered, undefined).pipe(
											Effect.asVoid,
										);
									}

									if (
										event.payload.type === "thread.erased" &&
										event.thread_id === "thread_erased"
									) {
										yield* Deferred.succeed(erasure_delivered, undefined).pipe(
											Effect.asVoid,
										);
									}

									if (
										event.payload.type === "thread.message_queued" &&
										event.thread_id === "thread_kept" &&
										event.payload.text === "Surviving later event"
									) {
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

						yield* Deferred.await(guidance_delivered);
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
							Stream.take(8),
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

			expect(output.events[0]).toMatchObject({
				journal_sequence: 1,
				payload: { type: "guidance.canonical.updated" },
				stream_id: "settings:guidance",
				thread_id: "settings/guidance",
			});
			expect(output.events[0]?.payload).not.toHaveProperty("content");
			expect(output.events.slice(1).map((event) => event.journal_sequence)).toEqual([
				2, 3, 4, 5, 6, 7, 8,
			]);
			expect(output.events.slice(1).map((event) => event.sequence)).toEqual([
				1, 1, 2, 2, 3, 4, 3,
			]);
			expect(output.events.slice(1).map((event) => event.thread_id)).toEqual([
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
				journal_sequence: 7,
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
					{ sequence: 1, stream_id: "settings:guidance" },
					{ sequence: 4, stream_id: "thread:thread_erased" },
					{ sequence: 3, stream_id: "thread:thread_kept" },
				],
				last_journal_sequence: 8,
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

import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Schedule } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope } from "@artisan/protocol";
import {
	make_backend_runtime,
	make_thread_metadata_refiner_test_layer,
	ProtocolRouter,
	ThreadMetadataRefinementCoordinator,
	ThreadMetadataRefinerLive,
	type ThreadMetadataRefinerInput,
} from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { EventStreams, JournalEvents } from "../../modules/backend/src/persistence/tables";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-refinement-coordinator-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_command(message_id: string, payload: CommandEnvelope["payload"]): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-11T14:00:00.000Z",
		thread_id: "thread_coordinator",
	};
}

const append_user_message = (journal: JournalStore["Service"], text: string, suffix: string) =>
	journal.AppendEvent({
		causation_id: `cause_${suffix}`,
		correlation_id: `correlation_${suffix}`,
		payload: {
			message_id: `user_message_${suffix}`,
			reason: "no_active_run",
			text,
			type: "thread.message_queued",
			working_directory: "C:/workspace/artisan",
		},
		thread_id: "thread_coordinator",
	});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread metadata refinement coordinator", () => {
	it("projects completed assistant prose without replacing lifecycle status", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			thread_metadata_refiner: ThreadMetadataRefinerLive,
		});

		try {
			const snapshots = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						make_command("create_assistant_preview", {
							title: "Assistant preview",
							type: "thread.create",
						}),
					);
					yield* append_user_message(journal, "Inspect the repository", "preview");
					const values = [
						"The repository is a Svelte/TypeScript app using `svelte-effect-runtime`.",
						"Complete",
						"Waiting for answer",
						"Failed to complete",
					];
					const projected = [];

					for (const [index, text] of values.entries()) {
						yield* journal.AppendEvent({
							causation_id: `assistant_preview_cause_${index}`,
							correlation_id: `assistant_preview_correlation_${index}`,
							payload: {
								message_id: `assistant_preview_message_${index}`,
								text,
								type: "assistant.message_completed",
							},
							thread_id: "thread_coordinator",
						});
						yield* coordinator.WaitForIdle;
						projected.push((yield* threads.Snapshot()).threads[0]!);
					}

					return projected;
				}),
			);

			expect(snapshots.map((thread) => thread.last_assistant_message)).toEqual([
				"The repository is a Svelte/TypeScript app using `svelte-effect-runtime`.",
				"Complete",
				"Waiting for answer",
				"Failed to complete",
			]);
			expect(snapshots.every((thread) => thread.live_status === "Working")).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps the assistant preview through terminal and subsequent run transitions", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			thread_metadata_refiner: ThreadMetadataRefinerLive,
		});

		try {
			const [completed, restarted] = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						make_command("create_ordered_preview", {
							title: "Ordered preview",
							type: "thread.create",
						}),
					);
					yield* journal.AppendEvent({
						causation_id: "ordered_assistant_cause",
						correlation_id: "ordered_assistant_correlation",
						payload: {
							message_id: "ordered_assistant_message",
							text: "The durable preview survives lifecycle changes.",
							type: "assistant.message_completed",
						},
						thread_id: "thread_coordinator",
					});
					yield* journal.AppendEvent({
						causation_id: "ordered_complete_cause",
						correlation_id: "ordered_complete_correlation",
						payload: {
							state: "completed",
							type: "run.lifecycle",
							working_directory: "C:/workspace/artisan",
						},
						run_id: "run_ordered",
						thread_id: "thread_coordinator",
					});
					yield* coordinator.WaitForIdle;
					const after_complete = (yield* threads.Snapshot()).threads[0]!;

					yield* journal.AppendEvent({
						causation_id: "ordered_restart_cause",
						correlation_id: "ordered_restart_correlation",
						payload: {
							state: "running",
							type: "run.lifecycle",
							working_directory: "C:/workspace/artisan",
						},
						run_id: "run_ordered_restart",
						thread_id: "thread_coordinator",
					});
					yield* coordinator.WaitForIdle;

					return [after_complete, (yield* threads.Snapshot()).threads[0]!] as const;
				}),
			);

			expect(completed).toMatchObject({
				last_assistant_message: "The durable preview survives lifecycle changes.",
				live_status: "Complete",
			});
			expect(restarted).toMatchObject({
				last_assistant_message: "The durable preview survives lifecycle changes.",
				live_status: "Working",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("cold-replays assistant prose followed by a terminal lifecycle", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;

					yield* router.Route(
						make_command("create_cold_preview", {
							title: "Cold preview",
							type: "thread.create",
						}),
					);
					yield* journal.AppendEvent({
						causation_id: "cold_assistant_cause",
						correlation_id: "cold_assistant_correlation",
						payload: {
							message_id: "cold_assistant_message",
							text: "Recovered assistant preview",
							type: "assistant.message_completed",
						},
						thread_id: "thread_coordinator",
					});
					yield* journal.AppendEvent({
						causation_id: "cold_complete_cause",
						correlation_id: "cold_complete_correlation",
						payload: {
							state: "completed",
							type: "run.lifecycle",
							working_directory: "C:/workspace/artisan",
						},
						run_id: "run_cold",
						thread_id: "thread_coordinator",
					});
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			thread_metadata_refiner: ThreadMetadataRefinerLive,
		});

		try {
			const thread = await second_runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;
					const threads = yield* ThreadReadModel;

					yield* coordinator.WaitForIdle;

					return (yield* threads.Snapshot()).threads[0]!;
				}),
			);

			expect(thread).toMatchObject({
				last_assistant_message: "Recovered assistant preview",
				live_status: "Complete",
			});
		} finally {
			await second_runtime.dispose();
		}
	});

	it("uses the latest meaningful trigger with accumulated provider-neutral context", async () => {
		const database_path = await make_database_path();
		const seen: ThreadMetadataRefinerInput[] = [];
		const refiner = make_thread_metadata_refiner_test_layer((input) =>
			Effect.sync(() => {
				seen.push(input);
				const latest_text = input.recent_user_text.at(-1)!;

				return {
					current_goal: latest_text,
					live_status: `Refined ${input.trigger}`,
					rename_suggestion: `Refined ${latest_text}`,
					title: `Refined ${latest_text}`,
				};
			}),
		);
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			thread_metadata_refiner: refiner,
		});

		try {
			const thread = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						make_command("create_coordinator", {
							title: "Initial coordinator title",
							type: "thread.create",
						}),
					);
					yield* journal.AppendEvent({
						causation_id: "artifact_cause",
						correlation_id: "artifact_correlation",
						payload: {
							artifact: {
								artifact_id: "artifact_file",
								assignment_id: "assignment_1",
								created_at: "2026-07-11T14:00:00.000Z",
								group_id: "group_1",
								kind: "file",
								label: "thread-metadata-refiner.ts",
								run_id: "run_1",
								uri: "C:/work/artisan/modules/backend/thread-metadata-refiner.ts",
							},
							group_id: "group_1",
							type: "artifact.recorded",
						},
						thread_id: "thread_coordinator",
					});
					yield* append_user_message(journal, "First direction", "first");
					yield* journal.AppendEvent({
						causation_id: "run_cause",
						correlation_id: "run_correlation",
						payload: {
							state: "running",
							type: "run.lifecycle",
							working_directory: "C:/workspace/artisan",
						},
						run_id: "run_1",
						thread_id: "thread_coordinator",
					});
					yield* journal.AppendEvent({
						causation_id: "steer_cause",
						correlation_id: "steer_correlation",
						payload: {
							message_id: "user_message_latest",
							text: "Latest direction",
							type: "thread.message_steering",
							working_directory: "C:/workspace/artisan",
						},
						thread_id: "thread_coordinator",
					});
					yield* coordinator.WaitForIdle;

					return (yield* threads.Snapshot()).threads[0]!;
				}),
			);
			const latest = seen.at(-1)!;
			const calls_after_idle = seen.length;

			expect(latest.trigger).toBe("user_message");
			expect(latest.recent_user_text).toEqual(["First direction", "Latest direction"]);
			expect(latest.recent_files).toContain(
				"C:/work/artisan/modules/backend/thread-metadata-refiner.ts",
			);
			expect(thread).toMatchObject({
				current_goal: "Latest direction",
				title: "Refined Latest direction",
				title_source: "automatic",
			});

			await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;

					yield* coordinator.WaitForIdle;
				}),
			);

			expect(seen).toHaveLength(calls_after_idle);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays a missed source after restart and skips it on later replays", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;

					yield* router.Route(
						make_command("create_replay", {
							title: "Before replay",
							type: "thread.create",
						}),
					);
					yield* append_user_message(journal, "Recovered direction", "replay");
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		const second_seen: ThreadMetadataRefinerInput[] = [];
		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			thread_metadata_refiner: make_thread_metadata_refiner_test_layer((input) =>
				Effect.sync(() => {
					second_seen.push(input);
					const latest_text = input.recent_user_text.at(-1)!;

					return {
						current_goal: latest_text,
						live_status: "Recovered",
						title: "Recovered thread title",
					};
				}),
			),
		});

		try {
			const title = await second_runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;
					const threads = yield* ThreadReadModel;

					yield* coordinator.WaitForIdle;

					return (yield* threads.Snapshot()).threads[0]!.title;
				}),
			);

			expect(second_seen).toHaveLength(1);
			expect(title).toBe("Recovered thread title");
		} finally {
			await second_runtime.dispose();
		}

		const third_seen: ThreadMetadataRefinerInput[] = [];
		const third_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			thread_metadata_refiner: make_thread_metadata_refiner_test_layer((input) =>
				Effect.sync(() => {
					third_seen.push(input);

					return { live_status: "Should not run" };
				}),
			),
		});

		try {
			await third_runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;

					yield* coordinator.WaitForIdle;
				}),
			);

			expect(third_seen).toHaveLength(0);
		} finally {
			await third_runtime.dispose();
		}
	});

	it("ignores unrelated and incompatible relevant history while refining later messages", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			thread_metadata_refiner: ThreadMetadataRefinerLive,
		});

		try {
			const title = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;
					const database = yield* Database;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						make_command("create_after_legacy_history", {
							title: "New thread",
							type: "thread.create",
						}),
					);
					yield* database.client.insert(JournalEvents).values([
						{
							causation_id: "legacy_unrelated_cause",
							correlation_id: "legacy_unrelated_correlation",
							event_id: "legacy_unrelated_event",
							event_type: "legacy.unrelated",
							occurred_at: "2026-07-11T14:00:01.000Z",
							origin: "backend",
							payload_json: JSON.stringify({ type: "legacy.unrelated" }),
							schema_version: 1,
							stream_id: "legacy:unrelated",
							stream_sequence: 1,
							thread_id: "settings/legacy",
						},
						{
							causation_id: "legacy_malformed_cause",
							correlation_id: "legacy_malformed_correlation",
							event_id: "legacy_malformed_event",
							event_type: "thread.message_queued",
							occurred_at: "2026-07-11T14:00:02.000Z",
							origin: "backend",
							payload_json: JSON.stringify({ type: "thread.message_queued" }),
							schema_version: 1,
							stream_id: "legacy:malformed",
							stream_sequence: 1,
							thread_id: "settings/legacy",
						},
						{
							causation_id: "historical_message_cause",
							correlation_id: "historical_message_correlation",
							event_id: "historical_message_event",
							event_type: "thread.message_queued",
							occurred_at: "2026-07-11T14:00:03.000Z",
							origin: "backend",
							payload_json: JSON.stringify({
								message_id: "historical_message",
								reason: "no_active_run",
								text: "Newest accepted direction",
								type: "thread.message_queued",
								working_directory: "C:/workspace/artisan",
							}),
							schema_version: 1,
							stream_id: "thread:thread_coordinator",
							stream_sequence: 2,
							thread_id: "thread_coordinator",
						},
					]);
					yield* database.client.update(EventStreams).set({ last_sequence: 2 });
					yield* coordinator.WaitForIdle;

					return (yield* threads.Snapshot()).threads[0]!.title;
				}),
			);

			expect(title).toBe("Newest accepted direction");
		} finally {
			await runtime.dispose();
		}
	});

	it("retries a failed refinement before advancing the replay cursor", async () => {
		const database_path = await make_database_path();
		let attempts = 0;
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			thread_metadata_refiner: make_thread_metadata_refiner_test_layer(() =>
				Effect.suspend(() => {
					attempts += 1;

					return attempts === 1
						? Effect.fail(new Error("temporary refiner failure"))
						: Effect.succeed({
								live_status: "Recovered",
								title: "Recovered after retry",
							});
				}),
			),
		});

		try {
			const title = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						make_command("create_retry", {
							title: "Before retry",
							type: "thread.create",
						}),
					);
					yield* append_user_message(journal, "Retry this refinement", "retry");
					yield* coordinator.WaitForIdle.pipe(
						Effect.retry({ schedule: Schedule.spaced("5 millis") }),
					);

					return (yield* threads.Snapshot()).threads[0]!.title;
				}),
			);

			expect(attempts).toBeGreaterThanOrEqual(2);
			expect(title).toBe("Recovered after retry");
		} finally {
			await runtime.dispose();
		}
	});
});

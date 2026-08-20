import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Exit, Layer, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope, EventPayload, ProjectRef } from "@artisan/protocol";
import {
	make_backend_runtime,
	make_thread_metadata_refiner_test_layer,
	ProjectLocator,
	ProtocolRouter,
	ThreadMetadataRefinementCoordinator,
	ThreadProjectAffinityCoordinator,
} from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import { ProjectCatalog } from "../../modules/backend/src/projects/project-catalog";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import {
	JournalConsumerCheckpoints,
	ThreadProjectAffinityEvidence,
} from "../../modules/backend/src/persistence/tables";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

const ProjectAlpha: ProjectRef = {
	display_name: "Alpha",
	project_id: "project_alpha",
	root_path: "C:/work/alpha",
};

const ProjectBeta: ProjectRef = {
	display_name: "Beta",
	project_id: "project_beta",
	root_path: "C:/work/beta",
};

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-affinity-coordinator-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_locator_layer() {
	return Layer.succeed(ProjectLocator, {
		Locate: (location) => {
			const project = location.includes("beta")
				? ProjectBeta
				: location.includes("alpha")
					? ProjectAlpha
					: undefined;

			return Effect.succeed(
				project === undefined
					? Option.none()
					: Option.some({ project, source: "git_root" as const }),
			);
		},
	});
}

function make_command(message_id: string, payload: CommandEnvelope["payload"]): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-11T15:00:00.000Z",
		thread_id: "thread_affinity_coordinator",
	};
}

const append_message = (journal: JournalStore["Service"], project: ProjectRef, suffix: string) =>
	journal.AppendEvent({
		causation_id: `cause_message_${suffix}`,
		correlation_id: `correlation_message_${suffix}`,
		payload: {
			message_id: `message_${suffix}`,
			reason: "no_active_run",
			text: `Work in ${project.display_name}`,
			type: "thread.message_queued",
			working_directory: project.root_path,
		},
		thread_id: "thread_affinity_coordinator",
	});

const append_run = (journal: JournalStore["Service"], project: ProjectRef, suffix: string) =>
	journal.AppendEvent({
		causation_id: `cause_run_${suffix}`,
		correlation_id: `correlation_run_${suffix}`,
		payload: {
			state: "running",
			type: "run.lifecycle",
			working_directory: project.root_path,
		},
		run_id: `run_${suffix}`,
		thread_id: "thread_affinity_coordinator",
	});

const append_evidence_event = (
	journal: JournalStore["Service"],
	payload: EventPayload,
	suffix: string,
) =>
	journal.AppendEvent({
		causation_id: `cause_evidence_${suffix}`,
		correlation_id: `correlation_evidence_${suffix}`,
		payload,
		thread_id: "thread_affinity_coordinator",
	});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread project affinity coordinator", () => {
	it("rejects a malformed historical stream before trusting notifier-driven tails", async () => {
		const database_path = await make_database_path();
		const preparation_runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			await preparation_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.run(
						"INSERT INTO event_streams (stream_id, last_sequence) VALUES ('gap_history', 3)",
					);
					yield* database.client.run(`
						INSERT INTO journal_events (
							stream_id, stream_sequence, schema_version, event_id, correlation_id,
							causation_id, origin, event_type, thread_id, payload_json, occurred_at
						) VALUES
							('gap_history', 1, 1, 'gap-history-1', 'gap-correlation-1',
								'gap-causation-1', 'backend', 'thread.created', 'thread_gap_history',
								'{"title":"Gap history","type":"thread.created"}', '2026-08-15T00:00:00.000Z'),
							('gap_history', 3, 1, 'gap-history-3', 'gap-correlation-3',
								'gap-causation-3', 'backend', 'thread.created', 'thread_gap_history',
								'{"title":"Gap history","type":"thread.created"}', '2026-08-15T00:00:00.000Z')
					`);
				}),
			);
		} finally {
			await preparation_runtime.dispose();
		}

		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			project_locator: make_locator_layer(),
		});

		try {
			const exits = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadProjectAffinityCoordinator;
					const database = yield* Database;
					const first = yield* coordinator.CatchUp.pipe(Effect.exit);
					const second = yield* coordinator.CatchUp.pipe(Effect.exit);

					return {
						checkpoints: yield* database.client
							.select()
							.from(JournalConsumerCheckpoints),
						first,
						second,
					};
				}),
			);

			expect([exits.first, exits.second].every(Exit.isFailure)).toBe(true);
			expect(exits.checkpoints).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("uses a persisted completed checkpoint after restart while still consuming later tails", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			project_locator: make_locator_layer(),
		});

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadProjectAffinityCoordinator;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;
					yield* router.Route(
						make_command("create_affinity_checkpoint", {
							title: "Checkpointed affinity evidence",
							type: "thread.create",
						}),
					);
					yield* append_run(journal, ProjectAlpha, "checkpoint_initial");
					yield* coordinator.CatchUp;
					expect(
						Option.isSome(
							yield* journal.ReadConsumerCheckpoint("thread-project-affinity"),
						),
					).toBe(true);
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		const preparation_runtime = make_backend_runtime({ database_path, migrations_path });
		try {
			await preparation_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.run(
						"UPDATE journal_events SET payload_json = '{malformed history' WHERE sequence = 1",
					);
				}),
			);
		} finally {
			await preparation_runtime.dispose();
		}

		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			project_locator: make_locator_layer(),
		});
		try {
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadProjectAffinityCoordinator;
					const database = yield* Database;
					const journal = yield* JournalStore;
					yield* append_run(journal, ProjectBeta, "checkpoint_tail");
					yield* coordinator.CatchUp;
					return {
						evidence: yield* database.client
							.select()
							.from(ThreadProjectAffinityEvidence),
					};
				}).pipe(Effect.timeout("2 seconds")),
			);
			expect(
				result.evidence.some((entry) => entry.project_id === ProjectBeta.project_id),
			).toBe(true);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("commits one monotonic checkpoint after sequential batch observation", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const journal = yield* JournalStore;
					yield* database.client.run(
						"CREATE TABLE checkpoint_write_counts (count integer NOT NULL)",
					);
					yield* database.client.run(
						"INSERT INTO checkpoint_write_counts (count) VALUES (0)",
					);
					yield* database.client.run(`
						CREATE TRIGGER count_checkpoint_insert
						AFTER INSERT ON journal_consumer_checkpoints
						BEGIN
							UPDATE checkpoint_write_counts SET count = count + 1;
						END;
					`);
					yield* journal.WriteConsumerCheckpoint("checkpoint-batch-test", 3);
					const write_rows = yield* database.client.all<{ readonly count: number }>(
						"SELECT count FROM checkpoint_write_counts",
					);
					const checkpoint =
						yield* journal.ReadConsumerCheckpoint("checkpoint-batch-test");
					yield* journal.WriteConsumerCheckpoint("checkpoint-batch-test", 0);

					return {
						checkpoint,
						checkpoint_after_stale_write:
							yield* journal.ReadConsumerCheckpoint("checkpoint-batch-test"),
						write_rows,
					};
				}),
			);

			const coordinator_source = await readFile(
				new URL(
					"../../modules/backend/src/threads/thread-project-affinity-coordinator.ts",
					import.meta.url,
				),
				"utf8",
			);

			expect(result.write_rows).toEqual([{ count: 1 }]);
			expect(Option.isSome(result.checkpoint)).toBe(true);
			expect(result.checkpoint).toEqual(result.checkpoint_after_stale_write);
			expect(coordinator_source).toMatch(
				/for \(const event of events\) yield\* ObserveEvent\(event\);[\s\S]*?WriteConsumerCheckpoint\(consumer_id, latest_journal_sequence\)/,
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("moves through linked, suggested, and rehomed states as recent ownership shifts", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			project_locator: make_locator_layer(),
		});

		try {
			const states = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadProjectAffinityCoordinator;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;
					const read = threads
						.Snapshot()
						.pipe(Effect.map((snapshot) => snapshot.threads[0]!));

					yield* router.Route(
						make_command("create_affinity_coordinator", {
							title: "Shift project ownership",
							type: "thread.create",
						}),
					);
					yield* append_message(journal, ProjectAlpha, "alpha_1");
					yield* append_run(journal, ProjectAlpha, "alpha_2");
					yield* coordinator.CatchUp;
					const alpha = yield* read;

					yield* append_message(journal, ProjectBeta, "beta_1");
					yield* append_run(journal, ProjectBeta, "beta_2");
					yield* coordinator.CatchUp;
					const intertwined = yield* read;

					yield* append_message(journal, ProjectBeta, "beta_3");
					yield* coordinator.CatchUp;
					const suggested = yield* read;

					yield* append_run(journal, ProjectBeta, "beta_4");
					yield* coordinator.CatchUp;
					const rehomed = yield* read;

					return { alpha, intertwined, rehomed, suggested };
				}),
			);

			expect(states.alpha).toMatchObject({
				primary_project: ProjectAlpha,
				project_locked: false,
			});
			expect(states.intertwined).toMatchObject({
				linked_projects: [ProjectBeta],
				primary_project: ProjectAlpha,
			});
			expect(states.intertwined).not.toHaveProperty("rehome_suggestion");
			expect(states.suggested).toMatchObject({
				primary_project: ProjectAlpha,
				rehome_suggestion: {
					project: ProjectBeta,
					score: 88,
				},
			});
			expect(states.rehomed).toMatchObject({
				linked_projects: [ProjectAlpha],
				primary_project: ProjectBeta,
			});
			expect(states.rehomed).not.toHaveProperty("rehome_suggestion");
		} finally {
			await runtime.dispose();
		}
	});

	it("persists observed evidence while locked and applies it only after manual unlock", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			project_locator: make_locator_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadProjectAffinityCoordinator;
					const database = yield* Database;
					const journal = yield* JournalStore;
					const projects = yield* ProjectCatalog;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						make_command("create_locked_coordinator", {
							title: "Locked project placement",
							type: "thread.create",
						}),
					);
					yield* projects.Attach(ProjectAlpha);
					yield* router.Route(
						make_command("assign_locked_alpha", {
							project_id: ProjectAlpha.project_id,
							type: "thread.project.assign",
						}),
					);
					yield* append_message(journal, ProjectBeta, "locked_beta_1");
					yield* append_run(journal, ProjectBeta, "locked_beta_2");
					yield* append_message(journal, ProjectBeta, "locked_beta_3");
					yield* append_run(journal, ProjectBeta, "locked_beta_4");
					yield* coordinator.CatchUp;
					const locked = (yield* threads.Snapshot()).threads[0]!;
					const evidence = yield* database.client
						.select()
						.from(ThreadProjectAffinityEvidence);
					yield* router.Route(
						make_command("unlock_after_observation", {
							basis_affinity_version: 1,
							type: "thread.project.unlock",
						}),
					);

					return {
						evidence,
						locked,
						unlocked: (yield* threads.Snapshot()).threads[0]!,
					};
				}),
			);

			expect(result.locked).toMatchObject({
				affinity_version: 1,
				primary_project: ProjectAlpha,
				project_locked: true,
			});
			expect(result.evidence).toHaveLength(10);
			expect(result.unlocked).toMatchObject({
				affinity_version: 2,
				linked_projects: [ProjectAlpha],
				primary_project: ProjectBeta,
				project_locked: false,
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("combines attributed filesystem, process, Git, mention, and metadata evidence", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			project_locator: make_locator_layer(),
			thread_metadata_refiner: make_thread_metadata_refiner_test_layer((input) =>
				Effect.succeed(
					input.recent_user_text.at(-1)?.includes("selected repository")
						? { mentioned_projects: [ProjectBeta] }
						: {},
				),
			),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadProjectAffinityCoordinator;
					const database = yield* Database;
					const journal = yield* JournalStore;
					const metadata_coordinator = yield* ThreadMetadataRefinementCoordinator;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						make_command("create_affinity_coordinator", {
							title: "Cross-repository implementation",
							type: "thread.create",
						}),
					);
					yield* append_run(journal, ProjectAlpha, "alpha_seed");
					yield* coordinator.CatchUp;

					yield* append_evidence_event(
						journal,
						{
							destination_path: `${ProjectBeta.root_path}/src/new.ts`,
							operation: "rename",
							path: `${ProjectBeta.root_path}/src/old.ts`,
							type: "filesystem.mutation",
						},
						"beta_filesystem",
					);
					yield* append_evidence_event(
						journal,
						{
							source: "artisan_tool",
							type: "process.ownership",
							working_directory: ProjectBeta.root_path,
						},
						"beta_process",
					);
					yield* append_evidence_event(
						journal,
						{
							branch: "feature/beta",
							changed_file_count: 3,
							has_diff: true,
							root_path: ProjectBeta.root_path,
							type: "git.workspace.observed",
							worktree_path: `${ProjectBeta.root_path}/.worktrees/feature-beta`,
						},
						"beta_git",
					);
					yield* append_evidence_event(
						journal,
						{
							mentioned_projects: [ProjectBeta],
							message_id: "message_beta_mention",
							reason: "no_active_run",
							text: "Continue in the selected repository.",
							type: "thread.message_queued",
							working_directory: ProjectAlpha.root_path,
						},
						"beta_mention",
					);
					yield* metadata_coordinator.WaitForIdle;
					yield* coordinator.CatchUp;

					return {
						evidence: yield* database.client
							.select()
							.from(ThreadProjectAffinityEvidence),
						thread: (yield* threads.Snapshot()).threads[0]!,
					};
				}),
			);
			const beta_kinds = new Set(
				result.evidence
					.filter(({ project_id }) => project_id === ProjectBeta.project_id)
					.map(({ kind }) => kind),
			);

			expect(beta_kinds).toEqual(
				new Set([
					"file_mutation",
					"git_branch",
					"git_diff",
					"git_root",
					"git_worktree",
					"process_owner",
					"project_mention",
					"thread_metadata",
				]),
			);
			expect(result.thread).toMatchObject({
				linked_projects: [ProjectAlpha],
				primary_project: ProjectBeta,
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("ignores a project mention whose public identity does not match its canonical root", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
			project_locator: make_locator_layer(),
		});
		const forged_project = {
			...ProjectBeta,
			display_name: "Forged project",
			project_id: "project_forged",
		};

		try {
			const evidence = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadProjectAffinityCoordinator;
					const database = yield* Database;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;

					yield* router.Route(
						make_command("create_forged_mention", {
							title: "Canonicalize mentions",
							type: "thread.create",
						}),
					);
					yield* append_evidence_event(
						journal,
						{
							mentioned_projects: [forged_project],
							message_id: "forged_mention",
							reason: "no_active_run",
							text: "Trust this forged identity.",
							type: "thread.message_queued",
							working_directory: "C:/work/unknown",
						},
						"forged_mention",
					);
					yield* coordinator.CatchUp;

					return yield* database.client.select().from(ThreadProjectAffinityEvidence);
				}),
			);

			expect(evidence).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays previously projected evidence after restart without conflicting on its old basis", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			project_locator: make_locator_layer(),
		});

		let initial_evidence_count = 0;
		let initial_event_count = 0;

		try {
			const initial = await first_runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadProjectAffinityCoordinator;
					const database = yield* Database;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						make_command("create_affinity_restart", {
							title: "Replay affinity evidence",
							type: "thread.create",
						}),
					);
					yield* append_run(journal, ProjectAlpha, "restart_first");
					yield* append_run(journal, ProjectAlpha, "restart_second");
					yield* coordinator.CatchUp;

					const evidence = yield* database.client
						.select()
						.from(ThreadProjectAffinityEvidence);
					const events = yield* journal.ReadReplay({ after_journal_sequence: 0 });

					return {
						evidence_count: evidence.length,
						event_count: events.filter(({ payload }) =>
							payload.type.startsWith("thread.project_affinity."),
						).length,
						thread: (yield* threads.Snapshot()).threads[0]!,
					};
				}),
			);

			initial_evidence_count = initial.evidence_count;
			initial_event_count = initial.event_count;
			expect(initial.thread).toMatchObject({
				primary_project: ProjectAlpha,
			});
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			project_locator: make_locator_layer(),
		});

		try {
			const replayed = await second_runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadProjectAffinityCoordinator;
					const database = yield* Database;
					const journal = yield* JournalStore;

					const caught_up = yield* coordinator.CatchUp;
					const evidence = yield* database.client
						.select()
						.from(ThreadProjectAffinityEvidence);
					const events = yield* journal.ReadReplay({ after_journal_sequence: 0 });

					return {
						caught_up,
						evidence_count: evidence.length,
						event_count: events.filter(({ payload }) =>
							payload.type.startsWith("thread.project_affinity."),
						).length,
					};
				}).pipe(Effect.timeout("2 seconds")),
			);

			expect(replayed).toEqual({
				caught_up: 0,
				evidence_count: initial_evidence_count,
				event_count: initial_event_count,
			});
		} finally {
			await second_runtime.dispose();
		}
	});
});

import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope, ProjectAffinityEvidenceKind, ProjectRef } from "@artisan/protocol";
import { make_backend_runtime, ProtocolRouter, ThreadRetentionClock } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import {
	JournalEvents,
	ThreadProjectAffinityEvidence,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { ProjectCatalog } from "../../modules/backend/src/projects/project-catalog";
import {
	ThreadProjectAffinityRepository,
	type ThreadProjectAffinityEvidenceInput,
} from "../../modules/backend/src/threads/thread-project-affinity-repository";

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
	const directory = await mkdtemp(join(tmpdir(), "artisan-project-affinity-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "project_affinity_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed("2026-07-11T12:00:00.000Z"),
	});
}

function make_retention_clock() {
	return Layer.succeed(ThreadRetentionClock, {
		Now: Effect.succeed("2026-07-11T12:00:00.000Z"),
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
		sent_at: "2026-07-11T12:00:00.000Z",
		thread_id: "thread_affinity",
	};
}

function make_create_command() {
	return make_command("create_affinity", {
		title: "Cross-repository work",
		type: "thread.create",
	});
}

function make_evidence(input: {
	readonly basis_affinity_version?: number;
	readonly evidence_id: string;
	readonly kind: ProjectAffinityEvidenceKind;
	readonly observed_at?: string;
	readonly project: ProjectRef;
	readonly source_event_id?: string;
	readonly source_journal_sequence?: number;
}): ThreadProjectAffinityEvidenceInput {
	return {
		basis_affinity_version: input.basis_affinity_version ?? 0,
		evidence_id: input.evidence_id,
		kind: input.kind,
		observed_at: input.observed_at ?? "2026-07-11T12:00:00.000Z",
		project: input.project,
		source_event_id: input.source_event_id ?? `source_${input.evidence_id}`,
		source_journal_sequence: input.source_journal_sequence ?? 1,
		thread_id: "thread_affinity",
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread project affinity repository", () => {
	it("progresses from observation to suggestion to a high-confidence automatic rehome", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const project_affinity = yield* ThreadProjectAffinityRepository;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(make_create_command());
					const observed = yield* project_affinity.ObserveEvidence(
						make_evidence({
							evidence_id: "evidence_git_root",
							kind: "git_root",
							project: ProjectAlpha,
							source_journal_sequence: 1,
						}),
					);
					const suggested = yield* project_affinity.ObserveEvidence(
						make_evidence({
							evidence_id: "evidence_file_mutation",
							kind: "file_mutation",
							project: ProjectAlpha,
							source_journal_sequence: 2,
						}),
					);
					const rehomed = yield* project_affinity.ObserveEvidence(
						make_evidence({
							evidence_id: "evidence_active_cwd",
							kind: "active_working_directory",
							project: ProjectAlpha,
							source_journal_sequence: 3,
						}),
					);
					const snapshot = yield* threads.Snapshot();
					const [row] = yield* database.client.select().from(Threads);

					return { observed, rehomed, row, snapshot, suggested };
				}),
			);
			const [thread] = result.snapshot.threads;

			expect(result.observed.event.payload).toMatchObject({
				change: "observed",
				type: "thread.project_affinity.updated",
			});
			expect(result.suggested.event.payload).toMatchObject({
				change: "suggested",
				thread: {
					affinity_version: 2,
					rehome_suggestion: {
						basis_affinity_version: 2,
						project: ProjectAlpha,
						score: 64,
					},
				},
				type: "thread.project_affinity.updated",
			});
			expect(result.rehomed.event.payload).toMatchObject({
				change: "rehomed",
				type: "thread.project_affinity.updated",
			});
			expect(thread).toMatchObject({
				affinity_version: 3,
				linked_projects: [],
				primary_project: ProjectAlpha,
				project_locked: false,
			});
			expect(thread).not.toHaveProperty("rehome_suggestion");
			expect(result.row?.primary_project_id).toBe(ProjectAlpha.project_id);
		} finally {
			await runtime.dispose();
		}
	});

	it("deduplicates evidence by source identity and rejects conflicting replays", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const project_affinity = yield* ThreadProjectAffinityRepository;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;
					const first_input = make_evidence({
						evidence_id: "evidence_replay_1",
						kind: "git_root",
						project: ProjectAlpha,
						source_event_id: "source_shared",
					});

					yield* router.Route(make_create_command());
					const first = yield* project_affinity.ObserveEvidence(first_input);
					const duplicate = yield* project_affinity.ObserveEvidence({
						...first_input,
						evidence_id: "evidence_replay_2",
					});
					const replayed_after_projection_change =
						yield* project_affinity.ObserveEvidence({
							...first_input,
							basis_affinity_version: 1,
							evidence_id: "evidence_replay_after_projection_change",
						});
					const conflict = yield* project_affinity
						.ObserveEvidence({
							...first_input,
							evidence_id: "evidence_replay_3",
							observed_at: "2026-07-11T12:01:00.000Z",
						})
						.pipe(Effect.exit);
					const future = yield* project_affinity
						.ObserveEvidence(
							make_evidence({
								basis_affinity_version: 3,
								evidence_id: "evidence_future",
								kind: "file_artifact",
								project: ProjectBeta,
							}),
						)
						.pipe(Effect.exit);

					return {
						conflict,
						duplicate,
						evidence: yield* database.client
							.select()
							.from(ThreadProjectAffinityEvidence),
						events: yield* database.client.select().from(JournalEvents),
						first,
						future,
						replayed_after_projection_change,
						snapshot: yield* threads.Snapshot(),
					};
				}),
			);
			const affinity_events = result.events.filter((event) =>
				event.event_type.startsWith("thread.project_affinity."),
			);

			expect(result.first.status).toBe("accepted");
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.duplicate.event).toEqual(result.first.event);
			expect(result.replayed_after_projection_change.status).toBe("duplicate");
			expect(result.replayed_after_projection_change.event).toEqual(result.first.event);
			expect(result.conflict._tag).toBe("Failure");
			expect(result.future._tag).toBe("Failure");
			expect(result.evidence).toHaveLength(1);
			expect(affinity_events).toHaveLength(1);
			expect(result.snapshot.threads[0]?.affinity_version).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("serializes concurrent evidence for one source fact into one event", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const project_affinity = yield* ThreadProjectAffinityRepository;
					const router = yield* ProtocolRouter;
					const shared = make_evidence({
						evidence_id: "concurrent_evidence_0",
						kind: "git_root",
						project: ProjectAlpha,
						source_event_id: "concurrent_source",
					});

					yield* router.Route(make_create_command());
					const acceptances = yield* Effect.all(
						Array.from({ length: 8 }, (_, index) =>
							project_affinity.ObserveEvidence({
								...shared,
								evidence_id: `concurrent_evidence_${index}`,
							}),
						),
						{ concurrency: "unbounded" },
					);
					const evidence = yield* database.client
						.select()
						.from(ThreadProjectAffinityEvidence);
					const events = yield* database.client.select().from(JournalEvents);

					return {
						acceptances,
						evidence,
						events: events.filter((event) =>
							event.event_type.startsWith("thread.project_affinity."),
						),
					};
				}),
			);

			expect(result.acceptances.filter(({ status }) => status === "accepted")).toHaveLength(
				1,
			);
			expect(result.acceptances.filter(({ status }) => status === "duplicate")).toHaveLength(
				7,
			);
			expect(result.evidence).toHaveLength(1);
			expect(result.events).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("persists locked evidence without moving and reevaluates it on a current unlock", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const project_affinity = yield* ThreadProjectAffinityRepository;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(make_create_command());
					const projects = yield* ProjectCatalog;
					yield* projects.Attach(ProjectAlpha);
					yield* router.Route(
						make_command("assign_alpha", {
							project_id: ProjectAlpha.project_id,
							type: "thread.project.assign",
						}),
					);
					const locked_events = yield* Effect.forEach(
						[
							["locked_git", "git_root"],
							["locked_file", "file_mutation"],
							["locked_cwd", "active_working_directory"],
						] as const,
						([evidence_id, kind], index) =>
							project_affinity.ObserveEvidence(
								make_evidence({
									basis_affinity_version: 1,
									evidence_id,
									kind,
									project: ProjectBeta,
									source_journal_sequence: index + 1,
								}),
							),
					);
					const while_locked = (yield* threads.Snapshot()).threads[0]!;
					const stale_unlock = yield* router.Route(
						make_command("unlock_stale", {
							basis_affinity_version: 0,
							type: "thread.project.unlock",
						}),
					);
					const unlocked = yield* router.Route(
						make_command("unlock_current", {
							basis_affinity_version: 1,
							type: "thread.project.unlock",
						}),
					);

					return {
						evidence: yield* database.client
							.select()
							.from(ThreadProjectAffinityEvidence),
						locked_events,
						stale_unlock,
						thread: (yield* threads.Snapshot()).threads[0]!,
						unlocked,
						while_locked,
					};
				}),
			);

			expect(result.locked_events.map((accepted) => accepted.event.payload)).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						reason: "locked",
						type: "thread.project_affinity.ignored",
					}),
				]),
			);
			expect(result.while_locked).toMatchObject({
				affinity_version: 1,
				primary_project: ProjectAlpha,
				project_locked: true,
			});
			expect(result.stale_unlock[1]).toMatchObject({
				payload: {
					reason: "stale_basis",
					type: "thread.project_affinity.ignored",
				},
			});
			expect(result.unlocked[1]).toMatchObject({
				payload: {
					change: "unlocked",
					type: "thread.project_affinity.updated",
				},
			});
			expect(result.thread).toMatchObject({
				affinity_version: 2,
				primary_project: ProjectBeta,
				project_locked: false,
			});
			expect(result.evidence).toHaveLength(3);
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps close cross-repository candidates linked in the durable projection", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const thread = await runtime.runPromise(
				Effect.gen(function* () {
					const project_affinity = yield* ThreadProjectAffinityRepository;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;
					const observations = [
						["alpha_git", "git_root", ProjectAlpha],
						["alpha_terminal", "terminal_working_directory", ProjectAlpha],
						["alpha_metadata", "thread_metadata", ProjectAlpha],
						["beta_file", "file_mutation", ProjectBeta],
						["beta_terminal", "terminal_working_directory", ProjectBeta],
						["beta_mention", "project_mention", ProjectBeta],
					] as const;

					yield* router.Route(make_create_command());
					yield* Effect.forEach(observations, ([evidence_id, kind, project]) =>
						project_affinity.ObserveEvidence(
							make_evidence({ evidence_id, kind, project }),
						),
					);

					return (yield* threads.Snapshot()).threads[0]!;
				}),
			);

			expect(thread.primary_project).toBeUndefined();
			expect(thread.rehome_suggestion).toBeUndefined();
			expect(thread.linked_projects).toEqual([ProjectAlpha, ProjectBeta]);
			expect(thread.project_affinity_scores.map((score) => score.score)).toEqual([56, 46]);
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps a manual assignment exact across retry, conflict, and restart", async () => {
		const database_path = await make_database_path();
		const assign = make_command("assign_retry", {
			project_id: ProjectAlpha.project_id,
			type: "thread.project.assign",
		});
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await first_runtime.runPromise(
				Effect.gen(function* () {
					const projects = yield* ProjectCatalog;
					const router = yield* ProtocolRouter;

					yield* router.Route(make_create_command());
					yield* projects.Attach(ProjectAlpha);
					yield* projects.Attach(ProjectBeta);
					const first = yield* router.Route(assign);
					const duplicate = yield* router.Route(assign);
					const conflict = yield* router.Route({
						...assign,
						payload: {
							project_id: ProjectBeta.project_id,
							type: "thread.project.assign",
						},
					});

					return { conflict, duplicate, first };
				}),
			);

			expect(result.first[0]).toMatchObject({ payload: { status: "accepted" } });
			expect(result.duplicate[0]).toMatchObject({ payload: { status: "duplicate" } });
			expect(result.duplicate[1]).toEqual(result.first[1]);
			expect(result.conflict[0]).toMatchObject({
				payload: {
					error: { code: "command.id_conflict" },
					status: "rejected",
				},
			});
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock: make_retention_clock(),
		});

		try {
			const thread = await second_runtime.runPromise(
				Effect.gen(function* () {
					const threads = yield* ThreadReadModel;

					return (yield* threads.Snapshot()).threads[0]!;
				}),
			);

			expect(thread).toMatchObject({
				affinity_version: 1,
				primary_project: ProjectAlpha,
				project_locked: true,
			});
		} finally {
			await second_runtime.dispose();
		}
	});
});

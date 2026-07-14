import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope, ProjectAffinityEvidenceKind, ProjectRef } from "@artisan/protocol";
import { make_backend_runtime, ProtocolRouter } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import {
	JournalEvents,
	JournalCommands,
	ThreadErasureClaims,
	ThreadProjectAffinityEvidence,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { ProjectRepository } from "../../modules/backend/src/projects/project-repository";
import {
	ThreadProjectAffinityRepository,
	ThreadProjectInitialAttachmentConflict,
	ThreadProjectInitialAttachmentProjectNotFound,
	type ThreadProjectAffinityEvidenceInput,
	type ThreadProjectInitialAttachmentInput,
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

const ProjectGamma: ProjectRef = {
	display_name: "Gamma",
	project_id: "project_gamma",
	root_path: "C:/work/gamma",
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

function make_initial_attachment(input: {
	readonly attachment_id: string;
	readonly project_id: string;
	readonly source_event_id?: string;
	readonly thread_id?: string;
}): ThreadProjectInitialAttachmentInput {
	return {
		attachment_id: input.attachment_id,
		project_id: input.project_id,
		source_event_id: input.source_event_id ?? `source_${input.attachment_id}`,
		thread_id: input.thread_id ?? "thread_affinity",
	};
}

function hosted_registration(project: ProjectRef) {
	const name = project.project_id.replace(/^project_/u, "");

	return {
		canonical_root: project.root_path,
		display_name: project.display_name,
		hosted_origin: {
			canonical_host: "github.com",
			clone_url: `git@github.com:artisan/${name}.git`,
			fetch_url: `git@github.com:artisan/${name}.git`,
			name,
			native_id: `R_${name}`,
			owner: "artisan",
			provider_id: "github",
			push_url: `git@github.com:artisan/${name}.git`,
			remote_name: "origin",
			selected_account_login: "sander",
			web_url: `https://github.com/artisan/${name}`,
		},
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
					yield* router.Route(
						make_command("assign_alpha", {
							project: ProjectAlpha,
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
			project: ProjectAlpha,
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
					const router = yield* ProtocolRouter;

					yield* router.Route(make_create_command());
					const first = yield* router.Route(assign);
					const duplicate = yield* router.Route(assign);
					const conflict = yield* router.Route({
						...assign,
						payload: {
							project: ProjectBeta,
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

		const second_runtime = make_backend_runtime({ database_path, migrations_path });

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

	it("attaches an initial registered project through one durable event without a command row", async () => {
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
					const projects = yield* ProjectRepository;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(make_create_command());
					const registered = yield* projects.RegisterHosted(
						hosted_registration(ProjectAlpha),
					);
					const accepted = yield* project_affinity.AttachInitialProject(
						make_initial_attachment({
							attachment_id: "initial_alpha",
							project_id: registered.project.project.project_id,
						}),
					);

					return {
						accepted,
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						project: registered.project.project,
						thread: (yield* threads.Snapshot()).threads[0]!,
					};
				}),
			);

			expect(result.accepted).toMatchObject({
				status: "accepted",
				event: {
					causation_id: "source_initial_alpha",
					payload: {
						change: "attached",
						type: "thread.project_affinity.updated",
					},
				},
			});
			expect(result.thread).toMatchObject({
				affinity_version: 1,
				linked_projects: [],
				primary_project: result.project,
				project_locked: false,
			});
			expect(result.commands.some((command) => command.message_id === "initial_alpha")).toBe(
				false,
			);
			expect(
				result.events.filter((event) => event.event_type.includes("project_affinity")),
			).toEqual([
				expect.objectContaining({
					causation_id: "source_initial_alpha",
					idempotency_key: "thread_project_initial_attachment:initial_alpha",
				}),
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays an initial attachment exactly across restart and rejects attachment-id reuse", async () => {
		const database_path = await make_database_path();
		const setup_runtime = make_backend_runtime({
			database_path,
			migrations_path,
		});
		const setup = await setup_runtime.runPromise(
			Effect.gen(function* () {
				const projects = yield* ProjectRepository;

				const alpha = yield* projects.RegisterHosted(hosted_registration(ProjectAlpha));
				const beta = yield* projects.RegisterHosted(hosted_registration(ProjectBeta));

				return {
					alpha: alpha.project.project,
					beta: beta.project.project,
				};
			}),
		);

		await setup_runtime.dispose();

		const attachment = make_initial_attachment({
			attachment_id: "initial_replay",
			project_id: setup.alpha.project_id,
		});
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const project_affinity = yield* ThreadProjectAffinityRepository;
					const router = yield* ProtocolRouter;

					yield* router.Route(make_create_command());
					const first = yield* project_affinity.AttachInitialProject(attachment);
					const duplicate = yield* project_affinity.AttachInitialProject(attachment);
					const project_conflict = yield* project_affinity
						.AttachInitialProject({ ...attachment, project_id: setup.beta.project_id })
						.pipe(Effect.flip);
					const source_conflict = yield* project_affinity
						.AttachInitialProject({
							...attachment,
							source_event_id: "different_attachment_source",
						})
						.pipe(Effect.flip);

					return {
						duplicate,
						events: yield* database.client.select().from(JournalEvents),
						first,
						project_conflict,
						source_conflict,
					};
				}),
			);

			expect(result.first.status).toBe("accepted");

			if (result.first.status !== "accepted") {
				throw new Error("The first attachment must create an event");
			}

			expect(result.duplicate).toEqual({
				event: result.first.event,
				status: "duplicate",
			});
			expect(result.project_conflict).toMatchObject({
				_tag: "CommandIdConflict",
				message_id: "initial_replay",
			});
			expect(result.source_conflict).toMatchObject({
				_tag: "CommandIdConflict",
				message_id: "initial_replay",
			});
			expect(result.events.filter((event) => event.idempotency_key !== null)).toHaveLength(1);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const replay = await second_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const project_affinity = yield* ThreadProjectAffinityRepository;
					const restarted = yield* project_affinity.AttachInitialProject(attachment);

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-14T10:01:00.000Z",
						thread_id: "thread_affinity",
					});

					return {
						erasing: yield* project_affinity.AttachInitialProject(attachment),
						restarted,
					};
				}),
			);

			expect(replay.restarted.status).toBe("duplicate");
			expect(replay.erasing.status).toBe("duplicate");
		} finally {
			await second_runtime.dispose();
		}
	});

	it("rejects initial-attachment replay with contradictory event routing identity", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const project_affinity = yield* ThreadProjectAffinityRepository;
					const projects = yield* ProjectRepository;
					const router = yield* ProtocolRouter;
					const registered = yield* projects.RegisterHosted(
						hosted_registration(ProjectAlpha),
					);
					const attachment = make_initial_attachment({
						attachment_id: "initial_routing_identity",
						project_id: registered.project.project.project_id,
					});

					yield* router.Route(make_create_command());
					yield* router.Route({
						...make_create_command(),
						message_id: "create_other_thread",
						thread_id: "thread_other",
					});
					yield* project_affinity.AttachInitialProject(attachment);
					const events = yield* database.client.select().from(JournalEvents);
					const attachment_event = events.find(
						(event) =>
							event.idempotency_key ===
							"thread_project_initial_attachment:initial_routing_identity",
					);

					if (!attachment_event) {
						return yield* Effect.die("The attachment event must be persisted");
					}

					yield* database.client
						.update(JournalEvents)
						.set({ correlation_id: "contradictory_attachment_id" });
					const correlation_conflict = yield* project_affinity
						.AttachInitialProject(attachment)
						.pipe(Effect.flip);

					yield* database.client.delete(JournalEvents);
					yield* database.client.insert(JournalEvents).values({
						...attachment_event,
						correlation_id: attachment.attachment_id,
						stream_id: "thread:thread_other",
					});
					const stream_conflict = yield* project_affinity
						.AttachInitialProject(attachment)
						.pipe(Effect.flip);

					return { correlation_conflict, stream_conflict };
				}),
			);

			expect(result.correlation_conflict).toMatchObject({
				_tag: "CommandIdConflict",
				message_id: "initial_routing_identity",
			});
			expect(result.stream_conflict).toMatchObject({
				_tag: "CommandIdConflict",
				message_id: "initial_routing_identity",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("converges concurrent initial-attachment retries across backend runtimes", async () => {
		const database_path = await make_database_path();
		const setup_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});
		const project = await setup_runtime.runPromise(
			Effect.gen(function* () {
				const projects = yield* ProjectRepository;
				const router = yield* ProtocolRouter;

				yield* router.Route(make_create_command());

				return (yield* projects.RegisterHosted(hosted_registration(ProjectAlpha))).project
					.project;
			}),
		);

		await setup_runtime.dispose();

		const attachment = make_initial_attachment({
			attachment_id: "concurrent_initial_attachment",
			project_id: project.project_id,
		});
		const left_runtime = make_backend_runtime({ database_path, migrations_path });
		const right_runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const Attach = (runtime: typeof left_runtime) =>
				runtime.runPromise(
					Effect.flatMap(ThreadProjectAffinityRepository, (repository) =>
						repository.AttachInitialProject(attachment),
					),
				);
			const [left, right] = await Promise.all([Attach(left_runtime), Attach(right_runtime)]);
			const events = await left_runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(JournalEvents),
				),
			);

			expect([left.status, right.status].toSorted()).toEqual(["accepted", "duplicate"]);
			expect(
				events.filter(
					(event) =>
						event.idempotency_key ===
						"thread_project_initial_attachment:concurrent_initial_attachment",
				),
			).toHaveLength(1);
		} finally {
			await Promise.all([left_runtime.dispose(), right_runtime.dispose()]);
		}
	});

	it("converges on an unlocked primary and refuses different, locked, missing, and erasing threads", async () => {
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
					const projects = yield* ProjectRepository;
					const router = yield* ProtocolRouter;
					const alpha = (yield* projects.RegisterHosted(
						hosted_registration(ProjectAlpha),
					)).project.project;
					const beta = (yield* projects.RegisterHosted(hosted_registration(ProjectBeta)))
						.project.project;

					const missing = yield* project_affinity
						.AttachInitialProject(
							make_initial_attachment({
								attachment_id: "missing_attachment",
								project_id: alpha.project_id,
								thread_id: "thread_missing",
							}),
						)
						.pipe(Effect.flip);

					yield* router.Route(make_create_command());
					const unknown = yield* project_affinity
						.AttachInitialProject(
							make_initial_attachment({
								attachment_id: "unknown_project_attachment",
								project_id: `project_${"f".repeat(64)}`,
							}),
						)
						.pipe(Effect.flip);
					yield* Effect.forEach(
						[
							["auto_alpha_git", "git_root"],
							["auto_alpha_file", "file_mutation"],
							["auto_alpha_cwd", "active_working_directory"],
						] as const,
						([evidence_id, kind], index) =>
							project_affinity.ObserveEvidence(
								make_evidence({
									evidence_id,
									kind,
									project: alpha,
									source_journal_sequence: index + 1,
								}),
							),
					);
					const convergence = yield* project_affinity.AttachInitialProject(
						make_initial_attachment({
							attachment_id: "converged_attachment",
							project_id: alpha.project_id,
						}),
					);
					const before_different = {
						event_count: (yield* database.client.select().from(JournalEvents)).length,
						thread: (yield* (yield* ThreadReadModel).Snapshot()).threads[0]!,
					};
					const different = yield* project_affinity
						.AttachInitialProject(
							make_initial_attachment({
								attachment_id: "different_attachment",
								project_id: beta.project_id,
							}),
						)
						.pipe(Effect.flip);
					const after_different = {
						event_count: (yield* database.client.select().from(JournalEvents)).length,
						thread: (yield* (yield* ThreadReadModel).Snapshot()).threads[0]!,
					};

					yield* router.Route(
						make_command("lock_alpha", {
							project: alpha,
							type: "thread.project.assign",
						}),
					);
					const before_locked = {
						event_count: (yield* database.client.select().from(JournalEvents)).length,
						thread: (yield* (yield* ThreadReadModel).Snapshot()).threads[0]!,
					};
					const locked = yield* project_affinity
						.AttachInitialProject(
							make_initial_attachment({
								attachment_id: "locked_attachment",
								project_id: alpha.project_id,
							}),
						)
						.pipe(Effect.flip);
					const after_locked = {
						event_count: (yield* database.client.select().from(JournalEvents)).length,
						thread: (yield* (yield* ThreadReadModel).Snapshot()).threads[0]!,
					};

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-11T12:01:00.000Z",
						thread_id: "thread_affinity",
					});
					const erasing = yield* project_affinity
						.AttachInitialProject(
							make_initial_attachment({
								attachment_id: "erasing_attachment",
								project_id: alpha.project_id,
							}),
						)
						.pipe(Effect.flip);
					const after_erasing = {
						event_count: (yield* database.client.select().from(JournalEvents)).length,
					};

					return {
						after_different,
						after_erasing,
						after_locked,
						before_different,
						before_locked,
						convergence,
						different,
						erasing,
						locked,
						missing,
						unknown,
					};
				}),
			);

			expect(result.convergence).toEqual({ status: "already_attached" });
			expect(result.different).toEqual(
				new ThreadProjectInitialAttachmentConflict({ thread_id: "thread_affinity" }),
			);
			expect(result.locked).toEqual(
				new ThreadProjectInitialAttachmentConflict({ thread_id: "thread_affinity" }),
			);
			expect(result.after_different).toEqual(result.before_different);
			expect(result.after_locked).toEqual(result.before_locked);
			expect(result.missing).toMatchObject({
				_tag: "ThreadProjectAffinityNotFound",
				thread_id: "thread_missing",
			});
			expect(result.unknown).toEqual(
				new ThreadProjectInitialAttachmentProjectNotFound({
					project_id: `project_${"f".repeat(64)}`,
				}),
			);
			expect(result.erasing).toMatchObject({
				_tag: "ThreadProjectAffinityNotFound",
				thread_id: "thread_affinity",
			});
			expect(result.after_erasing.event_count).toBe(result.after_locked.event_count);
		} finally {
			await runtime.dispose();
		}
	});

	it("preserves observed scores and linked projects while attaching the registered project", async () => {
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
					const projects = yield* ProjectRepository;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;
					const gamma = (yield* projects.RegisterHosted(
						hosted_registration(ProjectGamma),
					)).project.project;

					yield* router.Route(make_create_command());
					yield* Effect.forEach(
						[
							["linked_alpha", "git_root", ProjectAlpha],
							["linked_alpha_terminal", "terminal_working_directory", ProjectAlpha],
							["linked_alpha_metadata", "thread_metadata", ProjectAlpha],
							["linked_beta", "file_mutation", ProjectBeta],
							["linked_beta_terminal", "terminal_working_directory", ProjectBeta],
							["linked_beta_mention", "project_mention", ProjectBeta],
						] as const,
						([evidence_id, kind, project], index) =>
							project_affinity.ObserveEvidence(
								make_evidence({
									evidence_id,
									kind,
									project,
									source_journal_sequence: index + 1,
								}),
							),
					);
					const before = (yield* threads.Snapshot()).threads[0]!;

					yield* project_affinity.AttachInitialProject(
						make_initial_attachment({
							attachment_id: "initial_gamma",
							project_id: gamma.project_id,
						}),
					);

					return { after: (yield* threads.Snapshot()).threads[0]!, before, gamma };
				}),
			);

			expect(thread.after).toMatchObject({
				linked_projects: [ProjectAlpha, ProjectBeta],
				primary_project: thread.gamma,
				project_locked: false,
			});
			expect(thread.after.project_affinity_scores).toEqual(
				thread.before.project_affinity_scores,
			);
		} finally {
			await runtime.dispose();
		}
	});
});

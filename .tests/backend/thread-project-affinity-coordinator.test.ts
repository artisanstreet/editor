import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope, ProjectRef } from "@artisan/protocol";
import {
	make_backend_runtime,
	ProjectLocator,
	ProtocolRouter,
	ThreadProjectAffinityCoordinator,
} from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { ThreadProjectAffinityEvidence } from "../../modules/backend/src/persistence/schema";
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

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread project affinity coordinator", () => {
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
					score: 76,
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
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						make_command("create_locked_coordinator", {
							title: "Locked project placement",
							type: "thread.create",
						}),
					);
					yield* router.Route(
						make_command("assign_locked_alpha", {
							project: ProjectAlpha,
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
			expect(result.evidence).toHaveLength(8);
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
});

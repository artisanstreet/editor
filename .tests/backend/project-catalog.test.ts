import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, PubSub } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope, ProjectRef } from "@artisan/protocol";
import { make_backend_runtime, ProtocolRouter } from "@artisan/backend";

import { ProjectCatalog } from "../../modules/backend/src/projects/project-catalog";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

const ProjectAlpha: ProjectRef = {
	display_name: "Alpha",
	project_id: "project_alpha",
	root_path: "C:/work/alpha",
};

const MakeCreateCommand = (project_id: string): CommandEnvelope => ({
	kind: "command",
	message_id: "create_project_thread",
	origin: "frontend",
	payload: {
		project_id,
		title: "Project thread",
		type: "thread.create",
	},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-07-27T09:00:00.000Z",
	thread_id: "thread_project",
});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("Forge project catalog", () => {
	it("persists attached projects across runtime restart", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-project-catalog-"));
		temporary_directories.push(directory);
		const database_path = join(directory, "artisan.db");
		const first = make_backend_runtime({ database_path, migrations_path });

		try {
			await first.runPromise(
				Effect.gen(function* () {
					const projects = yield* ProjectCatalog;
					yield* projects.Attach(ProjectAlpha);
				}),
			);
		} finally {
			await first.dispose();
		}

		const second = make_backend_runtime({ database_path, migrations_path });
		try {
			const snapshot = await second.runPromise(
				Effect.gen(function* () {
					const projects = yield* ProjectCatalog;
					return yield* projects.Snapshot;
				}),
			);
			expect(snapshot.projects).toEqual([
				expect.objectContaining({
					display_name: "Alpha",
					project_id: "project_alpha",
					root_path: "C:/work/alpha",
				}),
			]);
		} finally {
			await second.dispose();
		}
	});

	it("publishes authoritative catalog replacements to live subscribers", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-project-subscription-"));
		temporary_directories.push(directory);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
		});

		try {
			const snapshot = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const projects = yield* ProjectCatalog;
						const subscription = yield* projects.Subscribe;
						yield* projects.Attach(ProjectAlpha);
						return yield* PubSub.take(subscription);
					}),
				),
			);
			expect(snapshot.projects).toEqual([expect.objectContaining(ProjectAlpha)]);
		} finally {
			await runtime.dispose();
		}
	});

	it("creates and assigns a thread atomically from a Forge-owned project id", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-project-thread-"));
		temporary_directories.push(directory);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
		});

		try {
			const thread = await runtime.runPromise(
				Effect.gen(function* () {
					const projects = yield* ProjectCatalog;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;
					yield* projects.Attach(ProjectAlpha);
					yield* router.Route(MakeCreateCommand(ProjectAlpha.project_id));
					return (yield* threads.Snapshot()).threads[0]!;
				}),
			);

			expect(thread).toMatchObject({
				affinity_version: 1,
				primary_project: ProjectAlpha,
				project_locked: true,
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects thread creation for a project id Forge does not own", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-project-reject-"));
		temporary_directories.push(directory);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
		});

		try {
			const output = await runtime.runPromise(
				Effect.gen(function* () {
					const router = yield* ProtocolRouter;
					return yield* router.Route(MakeCreateCommand("project_forged"));
				}),
			);
			expect(output[0]).toMatchObject({
				kind: "command.receipt",
				payload: { status: "rejected" },
			});
		} finally {
			await runtime.dispose();
		}
	});
});

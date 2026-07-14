import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Effect, Exit, FileSystem, Layer, ManagedRuntime, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { ProjectHostedOrigins, Projects } from "../../modules/backend/src/persistence/schema";
import {
	ProjectRepository,
	ProjectRepositoryConflict,
	ProjectRepositoryInvariant,
	ProjectRepositoryLive,
} from "../../modules/backend/src/projects/project-repository";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({ prefix: "artisan-projects-" });

	temporary_directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(database_path: string) {
	const metadata = Layer.succeed(RuntimeMetadata, {
		instance_id: "project_repository_test",
		MakeId: () => Effect.die("Project catalog IDs are deterministic"),
		Now: Effect.succeed("2026-07-14T10:00:00.000Z"),
	});
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		metadata,
		NodeCrypto.layer,
	);

	return ManagedRuntime.make(ProjectRepositoryLive.pipe(Layer.provideMerge(infrastructure)));
}

function registration(overrides: Record<string, unknown> = {}) {
	return {
		canonical_root: "C:/projects/artisan",
		display_name: "Artisan Editor",
		hosted_origin: {
			canonical_host: "github.com",
			clone_url: "git@github.com:artisan/editor.git",
			fetch_url: "git@github.com:artisan/editor.git",
			name: "editor",
			native_id: "R_kgDOartisan",
			owner: "artisan",
			provider_id: "github",
			push_url: "git@github.com:artisan/editor.git",
			remote_name: "origin",
			selected_account_login: "sander",
			web_url: "https://github.com/artisan/editor",
		},
		...overrides,
	};
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) return Cause.squash(exit.cause);

	throw new Error("Expected the effect to fail");
}

afterEach(async () => {
	const directories = temporary_directories.splice(0);

	await Effect.runPromise(
		Effect.forEach(
			directories,
			(directory) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(directory, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("ProjectRepository", () => {
	it("registers once, reuses native identity across roots, and survives restart", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first_runtime = make_runtime(database_path);

		const first = await first_runtime.runPromise(
			Effect.flatMap(ProjectRepository, (repository) =>
				repository.RegisterHosted(registration()),
			),
		);
		const replay = await first_runtime.runPromise(
			Effect.flatMap(ProjectRepository, (repository) =>
				repository.RegisterHosted(registration({ canonical_root: "C:/elsewhere/editor" })),
			),
		);

		expect(first.status).toBe("registered");
		expect(replay).toEqual({ project: first.project, status: "existing" });
		await first_runtime.dispose();

		const second_runtime = make_runtime(database_path);
		try {
			const found = await second_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* ProjectRepository;

					return {
						by_project_id: yield* repository.FindByProjectId({
							project_id: first.project.project.project_id,
						}),
						by_workspace_id: yield* repository.FindByWorkspaceId({
							workspace_id: first.project.workspace_id,
						}),
					};
				}),
			);

			expect(Option.getOrThrow(found.by_project_id)).toEqual(first.project);
			expect(Option.getOrThrow(found.by_workspace_id)).toEqual(first.project);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("converges concurrent registration of one hosted identity across runtimes", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const left_runtime = make_runtime(database_path);
		const right_runtime = make_runtime(database_path);

		try {
			await left_runtime.runPromise(
				Effect.flatMap(ProjectRepository, (repository) => repository.List),
			);
			await right_runtime.runPromise(
				Effect.flatMap(ProjectRepository, (repository) => repository.List),
			);

			const Register = (runtime: typeof left_runtime) =>
				runtime.runPromise(
					Effect.flatMap(ProjectRepository, (repository) =>
						repository.RegisterHosted(registration()),
					),
				);
			const [left, right] = await Promise.all([
				Register(left_runtime),
				Register(right_runtime),
			]);
			const projects = await left_runtime.runPromise(
				Effect.flatMap(ProjectRepository, (repository) => repository.List),
			);

			expect([left.status, right.status].toSorted()).toEqual(["existing", "registered"]);
			expect(left.project).toEqual(right.project);
			expect(projects).toEqual([left.project]);
		} finally {
			await Promise.all([left_runtime.dispose(), right_runtime.dispose()]);
		}
	});

	it("rejects root and coordinate collisions and fails closed on malformed input", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* ProjectRepository;

					yield* repository.RegisterHosted(registration());
					const root_conflict = yield* repository
						.RegisterHosted(
							registration({
								hosted_origin: {
									...registration().hosted_origin,
									native_id: "R_kgDOother",
									owner: "other-owner",
								},
							}),
						)
						.pipe(Effect.exit);
					const coordinate_conflict = yield* repository
						.RegisterHosted(
							registration({
								canonical_root: "C:/other",
								hosted_origin: {
									...registration().hosted_origin,
									native_id: "R_kgDOother",
								},
							}),
						)
						.pipe(Effect.exit);
					const malformed = yield* repository.RegisterHosted({}).pipe(Effect.exit);

					return { coordinate_conflict, malformed, root_conflict };
				}),
			);

			expect(failure_from(result.root_conflict)).toEqual(
				new ProjectRepositoryConflict({ reason: "canonical_root" }),
			);
			expect(failure_from(result.coordinate_conflict)).toEqual(
				new ProjectRepositoryConflict({ reason: "hosted_coordinate" }),
			);
			expect(failure_from(result.malformed)).not.toBeInstanceOf(ProjectRepositoryInvariant);
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed when a persisted project no longer satisfies the catalog schema", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const failure = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ProjectRepository;

					yield* repository.RegisterHosted(registration());
					yield* database.client
						.update(Projects)
						.set({ registered_at: "not-an-iso-timestamp" });

					return yield* repository.List.pipe(Effect.flip);
				}),
			);

			expect(failure).toBeInstanceOf(ProjectRepositoryInvariant);
		} finally {
			await runtime.dispose();
		}
	});

	it.each([
		{ field: "project_id", value: `project_${"f".repeat(64)}` },
		{ field: "workspace_id", value: `workspace_${"e".repeat(64)}` },
	] as const)(
		"fails closed when a valid-shaped $field disagrees with the hosted identity",
		async ({ field, value }) => {
			const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

			try {
				const failure = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const repository = yield* ProjectRepository;
						const registered = yield* repository.RegisterHosted(registration());
						const project = registered.project;
						const project_id =
							field === "project_id" ? value : project.project.project_id;
						const workspace_id =
							field === "workspace_id" ? value : project.workspace_id;

						yield* database.client.delete(ProjectHostedOrigins);
						yield* database.client.delete(Projects);
						yield* database.client.insert(Projects).values({
							canonical_root: project.project.root_path,
							display_name: project.project.display_name,
							project_id,
							registered_at: project.registered_at,
							updated_at: project.updated_at,
							workspace_id,
						});
						yield* database.client.insert(ProjectHostedOrigins).values({
							...project.hosted_origin,
							project_id,
						});

						return yield* repository.List.pipe(Effect.flip);
					}),
				);

				expect(failure).toBeInstanceOf(ProjectRepositoryInvariant);
			} finally {
				await runtime.dispose();
			}
		},
	);
});

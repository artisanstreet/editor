import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime, Path } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	GitProvider,
	GitProviderError,
	type GitProviderCloneRequest,
} from "../../modules/backend/src/git-provider/git-provider";
import { make_git_provider_registry_layer } from "../../modules/backend/src/git-provider/git-provider-registry";
import { make_workspace_git_execution_gate_layer } from "../../modules/backend/src/git/workspace-git-execution-gate";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { EventStreams, Threads } from "../../modules/backend/src/persistence/schema";
import {
	HostedProjectCloneCoordinator,
	HostedProjectCloneCoordinatorLive,
} from "../../modules/backend/src/projects/hosted-project-clone-coordinator";
import { make_hosted_project_clone_destination_layer } from "../../modules/backend/src/projects/hosted-project-clone-destination";
import {
	HostedProjectCloneRepository,
	HostedProjectCloneRepositoryLive,
} from "../../modules/backend/src/projects/hosted-project-clone-repository";
import {
	ProjectRepository,
	ProjectRepositoryLive,
} from "../../modules/backend/src/projects/project-repository";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { ThreadProjectAffinityRepositoryLive } from "../../modules/backend/src/threads/thread-project-affinity-repository";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const platform_layer = Layer.mergeAll(NodeFileSystem.layer, NodePath.layer);
const temporary_directories: Array<string> = [];

interface FakeProviderState {
	clone_calls: number;
	prepare_calls: number;
}

const TestPlatform = Effect.all({
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
}).pipe(Effect.provide(platform_layer));

async function make_temporary_root() {
	const { file_system } = await Effect.runPromise(TestPlatform);
	const root = await Effect.runPromise(
		file_system.makeTempDirectory({ prefix: "artisan-hosted-clone-coordinator-" }),
	);

	temporary_directories.push(root);

	return root;
}

function repository() {
	return {
		archived: false,
		clone_url: "https://github.com/artisan/editor.git",
		default_branch: { _tag: "known" as const, name: "main" },
		identity: {
			host: "github.com",
			name: "editor",
			owner: "artisan",
			provider_id: "github",
		},
		origin: {
			native_id: "repository_1",
			provider_id: "github",
			resource_kind: "repository" as const,
		},
		viewer_permission: "write" as const,
		visibility: "private" as const,
		web_url: "https://github.com/artisan/editor",
	};
}

function clone_request(destination_path: string, message_id = "clone_request_1") {
	return {
		destination_path,
		message_id,
		repository: repository(),
		selection: {
			account_login: "sander",
			host: "github.com",
			provider_id: "github",
		},
		sent_at: "2026-07-14T12:00:00.000Z",
		thread_id: "thread_1",
	};
}

function registration(canonical_root: string) {
	return {
		canonical_root,
		display_name: "editor",
		hosted_origin: {
			canonical_host: "github.com",
			clone_url: "https://github.com/artisan/editor.git",
			fetch_url: "https://github.com/artisan/editor.git",
			name: "editor",
			native_id: "repository_1",
			owner: "artisan",
			provider_id: "github",
			push_url: "https://github.com/artisan/editor.git",
			remote_name: "origin" as const,
			selected_account_login: "sander",
			web_url: "https://github.com/artisan/editor",
		},
	};
}

function make_provider(
	state: FakeProviderState,
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
) {
	return {
		Clone: (input) =>
			Effect.gen(function* () {
				state.clone_calls += 1;

				yield* file_system
					.makeDirectory(path_service.join(input.destination.canonical_root, ".git"))
					.pipe(
						Effect.mapError(
							() =>
								new GitProviderError({
									host: "github.com",
									operation: "clone_repository",
									provider_id: "github",
									reason: "clone_failed",
									retryable: false,
								}),
						),
					);

				return {
					canonical_root: input.destination.canonical_root,
					output_complete: true,
					repository: input.preparation.repository,
					type: "cloned" as const,
				};
			}),
		Descriptor: {
			capabilities: [{ _tag: "available" as const, capability: "clone_repository" as const }],
			display_name: "GitHub",
			provider_id: "github",
		},
		DiscoverRepositories: () => Effect.die("Discovery is outside clone coordinator tests"),
		Inspect: Effect.die("Inspection is outside clone coordinator tests"),
		PrepareClone: (input: GitProviderCloneRequest) =>
			Effect.sync(() => {
				state.prepare_calls += 1;

				return { repository: input.repository, selection: input.selection };
			}),
	} satisfies typeof GitProvider.Service;
}

function make_runtime(
	database_path: string,
	projects_root: string,
	provider: typeof GitProvider.Service,
) {
	let next_id = 0;
	let next_time = Date.parse("2026-07-14T12:00:00.000Z");

	const metadata = Layer.succeed(RuntimeMetadata, {
		instance_id: "hosted_clone_coordinator_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_clone_${++next_id}`),
		Now: Effect.sync(() => new Date(next_time++).toISOString()),
	});
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_workspace_git_execution_gate_layer({ database_path }),
		metadata,
		JournalNotifierLive,
		NodeCrypto.layer,
		NodeFileSystem.layer,
		NodePath.layer,
	);
	const project_repository = ProjectRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const clone_repository = HostedProjectCloneRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
	);
	const thread_projects = ThreadProjectAffinityRepositoryLive.pipe(
		Layer.provideMerge(project_repository),
	);
	const destination = make_hosted_project_clone_destination_layer({ projects_root }).pipe(
		Layer.provideMerge(infrastructure),
	);
	const provider_registry = make_git_provider_registry_layer([
		{ hosts: ["github.com"], provider },
	]);
	const services = Layer.mergeAll(
		project_repository,
		clone_repository,
		thread_projects,
		destination,
		provider_registry,
	);

	return ManagedRuntime.make(
		HostedProjectCloneCoordinatorLive.pipe(Layer.provideMerge(services)),
	);
}

const SeedThread = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(Threads).values({
		created_at: "2026-07-14T12:00:00.000Z",
		thread_id: "thread_1",
		title: "Clone",
		title_source: "initial",
		updated_at: "2026-07-14T12:00:00.000Z",
	});
	yield* database.client.insert(EventStreams).values({
		last_sequence: 0,
		stream_id: "thread:thread_1",
	});
});

afterEach(async () => {
	const { file_system } = await Effect.runPromise(TestPlatform);
	const directories = temporary_directories.splice(0);

	await Effect.runPromise(
		Effect.forEach(
			directories,
			(directory) => file_system.remove(directory, { recursive: true }),
			{ discard: true },
		),
	);
});

describe("HostedProjectCloneCoordinator", () => {
	it("clones once after approval, registers, attaches, and replays without provider work", async () => {
		const root = await make_temporary_root();
		const { file_system, path_service } = await Effect.runPromise(TestPlatform);
		const projects_root = path_service.join(root, "projects");
		const destination_path = path_service.join(projects_root, "editor");
		const database_path = path_service.join(root, "artisan.db");
		const state = { clone_calls: 0, prepare_calls: 0 };

		await Effect.runPromise(file_system.makeDirectory(destination_path, { recursive: true }));

		const runtime = make_runtime(
			database_path,
			projects_root,
			make_provider(state, file_system, path_service),
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const clones = yield* HostedProjectCloneCoordinator;
					const clone_repository = yield* HostedProjectCloneRepository;
					const projects = yield* ProjectRepository;

					yield* SeedThread;
					const requested = yield* clones.Request(clone_request(destination_path));
					const approved = yield* clones.Respond({
						approval_id: requested.approval.approval_id,
						approved: true,
						message_id: "clone_decision_1",
						sent_at: "2026-07-14T12:00:01.000Z",
						thread_id: "thread_1",
					});

					yield* clones.AwaitIdle;

					const settled = yield* clone_repository.Query({
						approval_id: requested.approval.approval_id,
						thread_id: "thread_1",
					});
					const replay = yield* clones.Request(clone_request(destination_path));
					const changed_intent = yield* clones
						.Request(clone_request(path_service.join(projects_root, "other-editor")))
						.pipe(Effect.flip);

					return {
						approved,
						changed_intent,
						projects: yield* projects.List,
						replay,
						settled,
					};
				}),
			);

			expect(result.approved.approval.state).toBe("approved");
			expect(result.settled.approval).toMatchObject({
				attachment: "attached",
				state: "applied",
			});
			expect(result.replay.approval).toEqual(result.settled.approval);
			expect(result.changed_intent).toMatchObject({
				_tag: "HostedProjectCloneConflict",
				reason: "request_conflict",
			});
			expect(result.projects).toHaveLength(1);
			expect(result.projects[0]!.project.root_path).toBe(
				await Effect.runPromise(file_system.realPath(destination_path)),
			);
			expect(state).toEqual({ clone_calls: 1, prepare_calls: 1 });
			expect(await Effect.runPromise(file_system.readDirectory(destination_path))).toEqual([
				".git",
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("reuses an existing hosted project before provider preparation or destination access", async () => {
		const root = await make_temporary_root();
		const { file_system, path_service } = await Effect.runPromise(TestPlatform);
		const projects_root = path_service.join(root, "projects");
		const existing_root = path_service.join(projects_root, "existing-editor");
		const database_path = path_service.join(root, "artisan.db");
		const state = { clone_calls: 0, prepare_calls: 0 };

		await Effect.runPromise(file_system.makeDirectory(existing_root, { recursive: true }));

		const runtime = make_runtime(
			database_path,
			projects_root,
			make_provider(state, file_system, path_service),
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const clones = yield* HostedProjectCloneCoordinator;
					const projects = yield* ProjectRepository;
					const request = clone_request(
						path_service.join(projects_root, "missing-destination"),
					);

					yield* SeedThread;
					const registered = yield* projects.RegisterHosted(registration(existing_root));
					const reused = yield* clones.Request(request);
					const replay = yield* clones.Request(request);

					return { registered, replay, reused };
				}),
			);

			expect(result.reused.approval).toMatchObject({
				attachment: "attached",
				destination_path: result.registered.project.project.root_path,
				project: result.registered.project.project,
				state: "reused",
			});
			expect(result.replay.approval).toEqual(result.reused.approval);
			expect(result.replay.status).toBe("duplicate");
			expect(state).toEqual({ clone_calls: 0, prepare_calls: 0 });
		} finally {
			await runtime.dispose();
		}
	});
});

import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Effect, Exit, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	HostedGitSnapshotService,
	HostedGitSnapshotServiceFailure,
	HostedGitSnapshotServiceLive,
} from "../../modules/backend/src/git-provider/hosted-git-snapshot-service";
import { HostedGitSnapshotRepositoryLive } from "../../modules/backend/src/git-provider/hosted-git-snapshot-repository";
import { GitProviderRegistry } from "../../modules/backend/src/git-provider/git-provider-registry";
import {
	GitProviderError,
	type GitProvider,
} from "../../modules/backend/src/git-provider/git-provider";
import {
	WorkspaceGitObserver,
	type WorkspaceGitObservation,
} from "../../modules/backend/src/git/workspace-git-observer";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { HostedGitSnapshots, Threads } from "../../modules/backend/src/persistence/schema";
import {
	ProjectRepository,
	ProjectRepositoryLive,
} from "../../modules/backend/src/projects/project-repository";
import { RuntimeMetadataLive } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-14T15:00:00.000Z";
const root_path = "C:/projects/artisan-editor";
const head = "a".repeat(40);

interface ObserverState {
	calls: number;
	observations: ReadonlyArray<WorkspaceGitObservation>;
}

interface ProviderState {
	calls: number;
	failure?: GitProviderError;
	invalid_response: boolean;
}

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-hosted-git-snapshot-service-",
	});

	yield* Effect.sync(() => temporary_directories.push(directory));

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

async function make_database_path() {
	return Effect.runPromise(MakeDatabasePath);
}

function observation(overrides: Partial<WorkspaceGitObservation> = {}): WorkspaceGitObservation {
	return {
		adapter_worktrees: [
			{
				adapter_path: root_path,
				bare: false,
				branch: "main",
				detached: false,
				head,
				locked: false,
				location: "selected",
				prunable: false,
			},
		],
		blockers: [],
		branch: "main",
		changed_files: [],
		diff_stats: { additions: 0, deletions: 0, files: 0 },
		has_diff: false,
		head,
		observed_at: now,
		repository_root: root_path,
		selected_worktree_path: root_path,
		state: "ready",
		workspace_id: "workspace_fixture",
		worktrees: [
			{
				bare: false,
				branch: "main",
				detached: false,
				head,
				locked: false,
				location: "selected",
				prunable: false,
			},
		],
		...overrides,
	};
}

function make_provider(state: ProviderState): typeof GitProvider.Service {
	return {
		Clone: () => Effect.die("unused"),
		Descriptor: {
			capabilities: [
				{ _tag: "available", capability: "read_reviews" },
				{ _tag: "available", capability: "read_ci" },
			],
			display_name: "GitHub",
			provider_id: "github",
		},
		DiscoverRepositories: () => Effect.die("unused"),
		Inspect: Effect.die("unused"),
		PrepareClone: () => Effect.die("unused"),
		ReadPullRequest: (input) => {
			state.calls += 1;

			if (state.failure !== undefined) {
				return Effect.fail(state.failure);
			}

			return Effect.succeed({
				association: { _tag: "none" },
				branch: input.selected_branch,
				expected_head_commit: state.invalid_response ? "f".repeat(40) : input.expected_head,
				repository: input.repository,
			});
		},
	};
}

function make_runtime(
	database_path: string,
	observer_state: ObserverState,
	provider_state: ProviderState,
) {
	const observer = Layer.succeed(WorkspaceGitObserver, {
		Observe: (workspace_id) =>
			Effect.sync(() => {
				const call = observer_state.calls++;
				const selected =
					observer_state.observations[call] ?? observer_state.observations.at(-1)!;

				return { ...selected, workspace_id };
			}),
	});
	const provider = make_provider(provider_state);
	const registry = Layer.succeed(GitProviderRegistry, {
		Get: (provider_id) =>
			provider_id === "github" ? Effect.succeed(provider) : Effect.die("unknown provider"),
		ResolveHost: () => Effect.die("unused"),
	});
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		RuntimeMetadataLive,
		JournalNotifierLive,
	);
	const project_repository = ProjectRepositoryLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(infrastructure),
	);
	const snapshot_repository = HostedGitSnapshotRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
	);
	const service = HostedGitSnapshotServiceLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(project_repository),
		Layer.provideMerge(snapshot_repository),
		Layer.provideMerge(observer),
		Layer.provideMerge(registry),
	);

	return ManagedRuntime.make(service);
}

async function seed_project(runtime: ManagedRuntime.ManagedRuntime<any, any>) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const database = yield* Database;
			const projects = yield* ProjectRepository;
			const registration = yield* projects.RegisterHosted({
				canonical_root: root_path,
				display_name: "Artisan Editor",
				hosted_origin: {
					canonical_host: "github.com",
					clone_url: "https://github.com/artisan/editor.git",
					fetch_url: "https://github.com/artisan/editor.git",
					name: "editor",
					native_id: "repository_1",
					owner: "artisan",
					provider_id: "github",
					push_url: "https://github.com/artisan/editor.git",
					remote_name: "origin",
					selected_account_login: "alice",
					web_url: "https://github.com/artisan/editor",
				},
			});

			yield* database.client.insert(Threads).values({
				created_at: now,
				primary_project_id: registration.project.project.project_id,
				primary_project_json: JSON.stringify(registration.project.project),
				thread_id: "thread_1",
				title: "Hosted state",
				title_source: "initial",
				updated_at: now,
			});

			return registration.project;
		}),
	);
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected the Effect to fail");
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

describe("HostedGitSnapshotService", () => {
	it("replays an accepted refresh across backend restart without reading the provider again", async () => {
		const database_path = await make_database_path();
		const observer_state = {
			calls: 0,
			observations: [observation(), observation()],
		} satisfies ObserverState;
		const provider_state = { calls: 0, invalid_response: false } satisfies ProviderState;
		const first_runtime = make_runtime(database_path, observer_state, provider_state);
		const registered = await seed_project(first_runtime);
		const command = {
			message_id: "hosted_refresh_1",
			sent_at: now,
			thread_id: "thread_1",
			workspace_id: registered.workspace_id,
		};

		const first = await first_runtime.runPromise(
			Effect.flatMap(HostedGitSnapshotService, (service) => service.Refresh(command)),
		);

		await first_runtime.dispose();

		const second_runtime = make_runtime(database_path, observer_state, provider_state);

		try {
			const duplicate = await second_runtime.runPromise(
				Effect.flatMap(HostedGitSnapshotService, (service) => service.Refresh(command)),
			);

			expect(duplicate).toEqual({ ...first, status: "duplicate" });
			expect(provider_state.calls).toBe(1);
			expect(observer_state.calls).toBe(2);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("does not persist a provider read when the visible branch changes during it", async () => {
		const observer_state = {
			calls: 0,
			observations: [observation(), observation({ branch: "release", head: "b".repeat(40) })],
		} satisfies ObserverState;
		const provider_state = { calls: 0, invalid_response: false } satisfies ProviderState;
		const runtime = make_runtime(await make_database_path(), observer_state, provider_state);

		try {
			const registered = await seed_project(runtime);
			const exit = await runtime.runPromise(
				Effect.flatMap(HostedGitSnapshotService, (service) =>
					Effect.exit(
						service.Refresh({
							message_id: "hosted_refresh_race",
							sent_at: now,
							thread_id: "thread_1",
							workspace_id: registered.workspace_id,
						}),
					),
				),
			);
			const rows = await runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(HostedGitSnapshots),
				),
			);

			expect(failure_from(exit)).toEqual(
				new HostedGitSnapshotServiceFailure({ reason: "branch_changed" }),
			);
			expect(rows).toEqual([]);
			expect(provider_state.calls).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects a provider response that is not bound to the observed local head", async () => {
		const observer_state = {
			calls: 0,
			observations: [observation()],
		} satisfies ObserverState;
		const provider_state = { calls: 0, invalid_response: true } satisfies ProviderState;
		const runtime = make_runtime(await make_database_path(), observer_state, provider_state);

		try {
			const registered = await seed_project(runtime);
			const exit = await runtime.runPromise(
				Effect.flatMap(HostedGitSnapshotService, (service) =>
					Effect.exit(
						service.Refresh({
							message_id: "hosted_refresh_invalid_provider",
							sent_at: now,
							thread_id: "thread_1",
							workspace_id: registered.workspace_id,
						}),
					),
				),
			);
			const rows = await runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(HostedGitSnapshots),
				),
			);

			expect(failure_from(exit)).toEqual(
				new HostedGitSnapshotServiceFailure({ reason: "invalid_provider_response" }),
			);
			expect(rows).toEqual([]);
			expect(provider_state.calls).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("preserves a provider authentication failure without persisting a snapshot", async () => {
		const provider_failure = new GitProviderError({
			host: "github.com",
			operation: "read_pull_request",
			provider_id: "github",
			reason: "auth_required",
			retryable: false,
		});
		const observer_state = {
			calls: 0,
			observations: [observation()],
		} satisfies ObserverState;
		const provider_state = {
			calls: 0,
			failure: provider_failure,
			invalid_response: false,
		} satisfies ProviderState;
		const runtime = make_runtime(await make_database_path(), observer_state, provider_state);

		try {
			const registered = await seed_project(runtime);
			const exit = await runtime.runPromise(
				Effect.flatMap(HostedGitSnapshotService, (service) =>
					Effect.exit(
						service.Refresh({
							message_id: "hosted_refresh_auth_required",
							sent_at: now,
							thread_id: "thread_1",
							workspace_id: registered.workspace_id,
						}),
					),
				),
			);
			const rows = await runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(HostedGitSnapshots),
				),
			);

			expect(failure_from(exit)).toEqual(provider_failure);
			expect(rows).toEqual([]);
			expect(provider_state.calls).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("marks a durable snapshot stale after the visible checkout head changes", async () => {
		const observer_state = {
			calls: 0,
			observations: [observation(), observation(), observation({ head: "c".repeat(40) })],
		} satisfies ObserverState;
		const provider_state = { calls: 0, invalid_response: false } satisfies ProviderState;
		const runtime = make_runtime(await make_database_path(), observer_state, provider_state);

		try {
			const registered = await seed_project(runtime);

			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* HostedGitSnapshotService;

					const accepted = yield* service.Refresh({
						message_id: "hosted_refresh_stale",
						sent_at: now,
						thread_id: "thread_1",
						workspace_id: registered.workspace_id,
					});

					const queried = yield* service.Query({ workspace_id: registered.workspace_id });

					return { accepted, queried };
				}),
			);

			expect(result.accepted.snapshot.workspace_freshness).toBe("unverified");
			expect(result.queried.snapshot?.workspace_freshness).toBe("stale_local_git");
			expect(result.queried.snapshot?.lookup.expected_head_commit).toBe(head);
		} finally {
			await runtime.dispose();
		}
	});
});

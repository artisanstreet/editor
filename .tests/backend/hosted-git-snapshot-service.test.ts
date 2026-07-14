import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Deferred, Effect, Exit, Fiber, FileSystem, Layer, ManagedRuntime } from "effect";
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
	detail_calls?: number;
	detail_gate?: {
		readonly entered: Deferred.Deferred<void>;
		readonly release: Deferred.Deferred<void>;
	};
	detail_head?: string;
	failure?: GitProviderError;
	invalid_response: boolean;
	matched?: boolean;
}

const pull_request_origin = {
	native_id: "PR_42",
	provider_id: "github",
	resource_kind: "pull_request" as const,
};
const check_origin = {
	native_id: "CR_7",
	provider_id: "github",
	resource_kind: "check_run" as const,
};
const workflow_origin = {
	native_id: "WR_9",
	provider_id: "github",
	resource_kind: "workflow_run" as const,
};

function matched_lookup(
	input: Parameters<NonNullable<(typeof GitProvider.Service)["ReadPullRequest"]>>[0],
) {
	return {
		association: {
			_tag: "matched" as const,
			freshness: "current" as const,
			pull_request: {
				base_branch: "main",
				base_commit: "b".repeat(40),
				checks: [
					{
						annotations: [],
						annotations_truncated: false,
						app_name: "GitHub Actions",
						attempt: 2,
						name: "test",
						origin: check_origin,
						required: true,
						state: "failed" as const,
						workflow_name: "CI",
						workflow_origin,
					},
				],
				checks_total: 1,
				checks_truncated: false,
				draft: false,
				head_branch: input.selected_branch,
				head_commit: input.expected_head,
				mergeability: "mergeable" as const,
				number: 42,
				origin: pull_request_origin,
				requested_reviewers: [],
				requested_reviewers_truncated: false,
				review_decision: "none" as const,
				review_threads: [],
				review_threads_total: 0,
				review_threads_truncated: false,
				reviews: [],
				reviews_total: 0,
				reviews_truncated: false,
				state: "open" as const,
				title: "Bound hosted failure detail",
				web_url: "https://github.com/artisan/editor/pull/42",
			},
		},
		branch: input.selected_branch,
		expected_head_commit: input.expected_head,
		repository: input.repository,
	};
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
		ReadCheckFailureDetail: (input) =>
			Effect.gen(function* () {
				state.detail_calls = (state.detail_calls ?? 0) + 1;

				if (state.detail_gate !== undefined) {
					yield* Deferred.succeed(state.detail_gate.entered, undefined);
					yield* Deferred.await(state.detail_gate.release);
				}

				return {
					attempt: 2,
					check_origin: input.check_origin,
					head_commit: state.detail_head ?? input.expected_head,
					log: {
						_tag: "available" as const,
						observed_bytes: 24,
						truncated: false,
						untrusted_excerpt: "private failure excerpt",
					},
					name: "test",
					output: {
						summary: {
							_tag: "available" as const,
							truncated: false,
							untrusted_text: "One failed job",
						},
						text: { _tag: "unavailable" as const },
					},
					workflow_origin,
				};
			}),
		ReadPullRequest: (input) => {
			state.calls += 1;

			if (state.failure !== undefined) {
				return Effect.fail(state.failure);
			}

			const lookup = state.matched
				? matched_lookup(input)
				: {
						association: { _tag: "none" as const },
						branch: input.selected_branch,
						expected_head_commit: input.expected_head,
						repository: input.repository,
					};

			return Effect.succeed({
				...lookup,
				expected_head_commit: state.invalid_response
					? "f".repeat(40)
					: lookup.expected_head_commit,
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
	it("returns exact-head failure detail without persisting provider output", async () => {
		const observer_state = {
			calls: 0,
			observations: [observation(), observation(), observation(), observation()],
		} satisfies ObserverState;
		const provider_state: ProviderState = {
			calls: 0,
			invalid_response: false,
			matched: true,
		};
		const runtime = make_runtime(await make_database_path(), observer_state, provider_state);

		try {
			const registered = await seed_project(runtime);
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const service = yield* HostedGitSnapshotService;
					const accepted = yield* service.Refresh({
						message_id: "hosted_refresh_failure_detail",
						sent_at: now,
						thread_id: "thread_1",
						workspace_id: registered.workspace_id,
					});
					const before_rows = yield* database.client.select().from(HostedGitSnapshots);
					const detail = yield* service.ReadCheckFailureDetail({
						check_origin,
						expected_head_commit: head,
						snapshot_version: accepted.snapshot.version,
						workspace_id: registered.workspace_id,
					});
					const after_rows = yield* database.client.select().from(HostedGitSnapshots);

					return { after_rows, before_rows, detail };
				}),
			);

			expect(result.detail.detail.log).toEqual({
				_tag: "available",
				observed_bytes: 24,
				truncated: false,
				untrusted_excerpt: "private failure excerpt",
			});
			expect(result.detail.snapshot_version).toBe(1);
			expect(result.after_rows).toEqual(result.before_rows);
			expect(JSON.stringify(result.after_rows)).not.toContain("private failure excerpt");
			expect(provider_state.detail_calls).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects stale snapshot intent before reading provider failure detail", async () => {
		const observer_state = {
			calls: 0,
			observations: [observation(), observation()],
		} satisfies ObserverState;
		const provider_state: ProviderState = {
			calls: 0,
			invalid_response: false,
			matched: true,
		};
		const runtime = make_runtime(await make_database_path(), observer_state, provider_state);

		try {
			const registered = await seed_project(runtime);
			const exit = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* HostedGitSnapshotService;
					const accepted = yield* service.Refresh({
						message_id: "hosted_refresh_stale_detail",
						sent_at: now,
						thread_id: "thread_1",
						workspace_id: registered.workspace_id,
					});

					return yield* Effect.exit(
						service.ReadCheckFailureDetail({
							check_origin,
							expected_head_commit: head,
							snapshot_version: accepted.snapshot.version + 1,
							workspace_id: registered.workspace_id,
						}),
					);
				}),
			);

			expect(failure_from(exit)).toEqual(
				new HostedGitSnapshotServiceFailure({ reason: "snapshot_stale" }),
			);
			expect(provider_state.detail_calls).toBeUndefined();
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects failure detail when the checkout changes during the provider read", async () => {
		const observer_state = {
			calls: 0,
			observations: [
				observation(),
				observation(),
				observation(),
				observation({ head: "c".repeat(40) }),
			],
		} satisfies ObserverState;
		const provider_state: ProviderState = {
			calls: 0,
			invalid_response: false,
			matched: true,
		};
		const runtime = make_runtime(await make_database_path(), observer_state, provider_state);

		try {
			const registered = await seed_project(runtime);
			const exit = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* HostedGitSnapshotService;
					const accepted = yield* service.Refresh({
						message_id: "hosted_refresh_racing_detail",
						sent_at: now,
						thread_id: "thread_1",
						workspace_id: registered.workspace_id,
					});

					return yield* Effect.exit(
						service.ReadCheckFailureDetail({
							check_origin,
							expected_head_commit: head,
							snapshot_version: accepted.snapshot.version,
							workspace_id: registered.workspace_id,
						}),
					);
				}),
			);

			expect(failure_from(exit)).toEqual(
				new HostedGitSnapshotServiceFailure({ reason: "branch_changed" }),
			);
			expect(provider_state.detail_calls).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects failure detail when the durable snapshot refreshes during the provider read", async () => {
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const observer_state = {
			calls: 0,
			observations: Array.from({ length: 6 }, () => observation()),
		} satisfies ObserverState;
		const provider_state: ProviderState = {
			calls: 0,
			detail_gate: { entered, release },
			invalid_response: false,
			matched: true,
		};
		const runtime = make_runtime(await make_database_path(), observer_state, provider_state);

		try {
			const registered = await seed_project(runtime);
			const exit = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* HostedGitSnapshotService;
					const accepted = yield* service.Refresh({
						message_id: "hosted_refresh_before_detail_race",
						sent_at: now,
						thread_id: "thread_1",
						workspace_id: registered.workspace_id,
					});
					const detail_fiber = yield* Effect.forkChild(
						service.ReadCheckFailureDetail({
							check_origin,
							expected_head_commit: head,
							snapshot_version: accepted.snapshot.version,
							workspace_id: registered.workspace_id,
						}),
					);

					yield* Deferred.await(entered);
					yield* service.Refresh({
						message_id: "hosted_refresh_during_detail_race",
						sent_at: now,
						thread_id: "thread_1",
						workspace_id: registered.workspace_id,
					});
					yield* Deferred.succeed(release, undefined);

					return yield* Fiber.await(detail_fiber);
				}),
			);

			expect(failure_from(exit)).toEqual(
				new HostedGitSnapshotServiceFailure({ reason: "snapshot_stale" }),
			);
			expect(provider_state.detail_calls).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

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

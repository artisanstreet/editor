import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Deferred, Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { TestClock } from "effect/testing";
import { afterEach, describe, expect, it } from "vitest";

import type { HostedGitCheck, HostedGitPullRequestLookup } from "@artisan/protocol";

import {
	ExternalWaitCoordinator,
	ExternalWaitCoordinatorLive,
	ExternalWaitScheduler,
} from "../../modules/backend/src/external-wait/external-wait-coordinator";
import { BuildExternalWaitBaseline } from "../../modules/backend/src/external-wait/external-wait-policy";
import {
	ExternalWaitRepository,
	ExternalWaitRepositoryLive,
} from "../../modules/backend/src/external-wait/external-wait-repository";
import {
	GitProviderError,
	type GitProvider,
	type GitProviderPullRequestTargetRead,
} from "../../modules/backend/src/git-provider/git-provider";
import { make_git_provider_registry_layer } from "../../modules/backend/src/git-provider/git-provider-registry";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	ExternalWaits,
	OrchestrationRuns,
	ProjectHostedOrigins,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import {
	ProjectRepository,
	ProjectRepositoryLive,
} from "../../modules/backend/src/projects/project-repository";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const initial_now = "2026-07-14T15:00:00.000Z";
const later_now = "2026-07-14T15:00:15.000Z";
const terminal_now = "2026-07-14T15:00:30.000Z";
const takeover_now = "2026-07-14T15:00:46.000Z";
const head = "b".repeat(40);

const repository = {
	host: "github.com",
	name: "editor",
	owner: "artisan",
	provider_id: "github",
} as const;

const target = {
	branch: "main",
	expected_head_commit: head,
	pull_request_number: 7,
	pull_request_origin: {
		native_id: "pr_7",
		provider_id: "github",
		resource_kind: "pull_request",
	},
	repository,
} as const;

interface ClockState {
	value: string;
}

interface ProviderState {
	readonly calls: Array<GitProviderPullRequestTargetRead>;
	failure?: GitProviderError;
	lookup: HostedGitPullRequestLookup;
	read?: () => Effect.Effect<HostedGitPullRequestLookup, GitProviderError>;
}

interface SchedulerState {
	active: number;
}

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-external-wait-coordinator-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function check(state: HostedGitCheck["state"]): HostedGitCheck {
	return {
		annotations: [],
		annotations_truncated: false,
		name: "build",
		origin: {
			native_id: "check_1",
			provider_id: "github",
			resource_kind: "check_run",
		},
		required: true,
		state,
	};
}

function lookup(
	check_state: HostedGitCheck["state"],
	freshness: "current" | "stale_head" = "current",
): HostedGitPullRequestLookup {
	return {
		association: {
			_tag: "matched" as const,
			freshness,
			pull_request: {
				base_branch: "main",
				base_commit: "a".repeat(40),
				checks: [check(check_state)],
				checks_total: 1,
				checks_truncated: false,
				draft: false,
				head_branch: "main",
				head_commit: head,
				mergeability: "mergeable" as const,
				number: 7,
				origin: target.pull_request_origin,
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
				title: "External wait",
				web_url: "https://github.com/artisan/editor/pull/7",
			},
		},
		branch: "main",
		expected_head_commit: head,
		repository,
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
		ReadPullRequestTarget: (input) =>
			Effect.sync(() => state.calls.push(input)).pipe(
				Effect.andThen(
					state.read?.() ??
						(state.failure === undefined
							? Effect.succeed(state.lookup)
							: Effect.fail(state.failure)),
				),
			),
	};
}

function metadata_layer(clock: ClockState, instance_id: string) {
	let identifier = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) =>
			Effect.sync(() => `${prefix}_external_wait_coordinator_${++identifier}`),
		Now: Effect.sync(() => clock.value),
	});
}

function runtime(
	database_path: string,
	clock: ClockState,
	provider_state: ProviderState,
	options: {
		readonly instance_id?: string;
		readonly scheduler?: SchedulerState;
	} = {},
) {
	const scheduler_state = options.scheduler ?? { active: 0 };
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		metadata_layer(clock, options.instance_id ?? "external_wait_coordinator_test"),
		JournalNotifierLive,
		NodeCrypto.layer,
	);
	const projects = ProjectRepositoryLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(infrastructure),
	);
	const external_waits = ExternalWaitRepositoryLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(infrastructure),
	);
	const providers = make_git_provider_registry_layer([
		{ hosts: ["github.com"], provider: make_provider(provider_state) },
	]);
	const scheduler = Layer.succeed(ExternalWaitScheduler, {
		Schedule: () =>
			Effect.acquireRelease(
				Effect.sync(() => {
					scheduler_state.active += 1;
				}),
				() =>
					Effect.sync(() => {
						scheduler_state.active -= 1;
					}),
			).pipe(Effect.andThen(Effect.never)),
	});
	const coordinator = ExternalWaitCoordinatorLive.pipe(
		Layer.provideMerge(external_waits),
		Layer.provideMerge(projects),
		Layer.provideMerge(providers),
		Layer.provideMerge(scheduler),
		Layer.provideMerge(TestClock.layer()),
		Layer.provideMerge(infrastructure),
	);

	return ManagedRuntime.make(coordinator);
}

const SeedWait = Effect.gen(function* () {
	const database = yield* Database;
	const external_waits = yield* ExternalWaitRepository;
	const projects = yield* ProjectRepository;
	const registered = yield* projects.RegisterHosted({
		canonical_root: "C:/artisan",
		display_name: "Artisan",
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
			selected_account_login: "sander",
			web_url: "https://github.com/artisan/editor",
		},
	});

	yield* database.client.insert(Threads).values({
		created_at: initial_now,
		primary_project_id: registered.project.project.project_id,
		primary_project_json: JSON.stringify(registered.project.project),
		thread_id: "thread_1",
		title: "External wait",
		title_source: "initial",
		updated_at: initial_now,
	});
	yield* database.client.insert(OrchestrationRuns).values({
		agent_id: "agent_1",
		created_at: initial_now,
		engine_id: "codex",
		run_id: "run_1",
		status: "running",
		thread_id: "thread_1",
		updated_at: initial_now,
		working_directory: "C:/artisan",
	});

	const registration = yield* BuildExternalWaitBaseline({
		gates: [{ _tag: "required_checks_terminal" }],
		lookup: lookup("running"),
		target,
	});

	if (registration._tag !== "usable") {
		return yield* Effect.die("Expected a usable external wait baseline");
	}

	yield* external_waits.Register({
		baseline: registration.baseline,
		owner: {
			_tag: "thread_run",
			agent_id: "agent_1",
			engine_id: "codex",
			run_id: "run_1",
		},
		project_id: registered.project.project.project_id,
		request: {
			expected_head_commit: head,
			gates: [{ _tag: "required_checks_terminal" }],
			pull_request_number: 7,
			source_run_id: "run_1",
			workspace_id: registered.project.workspace_id,
		},
		request_fingerprint: "b".repeat(64),
		source_command: { message_id: "command_1", sent_at: initial_now },
		target,
		thread_id: "thread_1",
		wait_id: "wait_1",
	});
});

const RunCoordinatorOnce = Effect.flatMap(
	ExternalWaitCoordinator,
	(coordinator) => coordinator.RunOnce,
);

afterEach(async () => {
	await Effect.runPromise(
		Effect.forEach(
			directories.splice(0),
			(directory) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(directory, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("ExternalWaitCoordinator", () => {
	it("reads the exact target, releases no-change, and wakes after terminal source recovery", async () => {
		const clock = { value: initial_now } satisfies ClockState;
		const provider_state = {
			calls: [],
			lookup: lookup("running"),
		} satisfies ProviderState;
		const instance = runtime(await Effect.runPromise(MakeDatabasePath), clock, provider_state);

		try {
			await instance.runPromise(SeedWait);
			clock.value = later_now;

			const first = await instance.runPromise(
				Effect.flatMap(ExternalWaitCoordinator, (coordinator) => coordinator.RunOnce),
			);
			const first_query = await instance.runPromise(
				Effect.flatMap(ExternalWaitRepository, (external_waits) =>
					external_waits.Query({ thread_id: "thread_1" }),
				),
			);

			expect(first).toEqual({ observed_wait_ids: ["wait_1"], reconciled_wait_ids: [] });
			expect(first_query.snapshots[0]?.state).toEqual({ _tag: "waiting" });
			expect(first_query.snapshots[0]?.version).toBe(1);
			expect(provider_state.calls).toEqual([
				{
					expected_head: head,
					pull_request_number: 7,
					pull_request_origin: target.pull_request_origin,
					repository,
					selected_branch: "main",
					selection: {
						account_login: "sander",
						host: "github.com",
						provider_id: "github",
					},
				},
			]);

			clock.value = terminal_now;
			provider_state.lookup = lookup("passed");
			await instance.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client
						.update(OrchestrationRuns)
						.set({ status: "completed", updated_at: terminal_now }),
				),
			);

			const second = await instance.runPromise(
				Effect.flatMap(ExternalWaitCoordinator, (coordinator) => coordinator.RunOnce),
			);
			const second_query = await instance.runPromise(
				Effect.flatMap(ExternalWaitRepository, (external_waits) =>
					external_waits.Query({ thread_id: "thread_1" }),
				),
			);
			const wakes = await instance.runPromise(
				Effect.flatMap(ExternalWaitRepository, (external_waits) =>
					external_waits.DiscoverWakes({ now: terminal_now }),
				),
			);

			expect(second.reconciled_wait_ids).toEqual(["wait_1"]);
			expect(second.observed_wait_ids).toEqual(["wait_1"]);
			expect(second_query.snapshots[0]?.state._tag).toBe("wake_pending");
			expect(wakes).toHaveLength(1);
			expect(provider_state.calls).toHaveLength(2);
		} finally {
			await instance.dispose();
		}
	});

	it("persists stale-head and authentication failures as canonical suspensions", async () => {
		const scenarios = [
			{
				expected: "stale_head",
				failure: undefined,
				lookup: lookup("running", "stale_head"),
			},
			{
				expected: "authentication_required",
				failure: new GitProviderError({
					host: "github.com",
					operation: "read_pull_request_target",
					provider_id: "github",
					reason: "auth_required",
					retryable: false,
				}),
				lookup: lookup("running"),
			},
		] as const;

		for (const scenario of scenarios) {
			const clock = { value: initial_now } satisfies ClockState;
			const provider_state: ProviderState = {
				calls: [],
				...(scenario.failure === undefined ? {} : { failure: scenario.failure }),
				lookup: scenario.lookup,
			};
			const instance = runtime(
				await Effect.runPromise(MakeDatabasePath),
				clock,
				provider_state,
			);

			try {
				await instance.runPromise(SeedWait);
				clock.value = later_now;
				await instance.runPromise(
					Effect.flatMap(ExternalWaitCoordinator, (coordinator) => coordinator.RunOnce),
				);

				const query = await instance.runPromise(
					Effect.flatMap(ExternalWaitRepository, (external_waits) =>
						external_waits.Query({ thread_id: "thread_1" }),
					),
				);
				const stored = await instance.runPromise(
					Effect.flatMap(Database, (database) =>
						database.client.select().from(ExternalWaits),
					),
				);

				expect(query.snapshots[0]?.state).toEqual({
					_tag: "suspended",
					reason: scenario.expected,
				});
				expect(stored[0]?.observer_lease_owner).toBeNull();
			} finally {
				await instance.dispose();
			}
		}
	});

	it("serializes overlapping cycles from one runtime", async () => {
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const clock = { value: initial_now } satisfies ClockState;
		const provider_state = {
			calls: [],
			lookup: lookup("running"),
			read: () =>
				Deferred.succeed(started, undefined).pipe(
					Effect.andThen(Deferred.await(release)),
					Effect.as(lookup("running")),
				),
		} satisfies ProviderState;
		const instance = runtime(await Effect.runPromise(MakeDatabasePath), clock, provider_state);

		try {
			await instance.runPromise(SeedWait);
			clock.value = later_now;

			const first = instance.runPromise(RunCoordinatorOnce);

			await instance.runPromise(Deferred.await(started));

			const second = instance.runPromise(RunCoordinatorOnce);

			await instance.runPromise(Deferred.succeed(release, undefined));

			const results = await Promise.all([first, second]);
			const observed = results.flatMap((result) => result.observed_wait_ids);

			expect(observed).toEqual(["wait_1"]);
			expect(provider_state.calls).toHaveLength(1);
		} finally {
			await instance.dispose();
		}
	});

	it("takes over an expired observation lease without duplicating the provider read", async () => {
		const clock = { value: initial_now } satisfies ClockState;
		const provider_state = {
			calls: [],
			lookup: lookup("running"),
		} satisfies ProviderState;
		const instance = runtime(await Effect.runPromise(MakeDatabasePath), clock, provider_state, {
			instance_id: "replacement_observer",
		});

		try {
			await instance.runPromise(SeedWait);
			await instance.runPromise(
				Effect.flatMap(ExternalWaitRepository, (external_waits) =>
					external_waits.ClaimObservation({
						lease_owner: "stopped_observer",
						now: later_now,
						wait_id: "wait_1",
					}),
				),
			);

			clock.value = takeover_now;

			const cycle = await instance.runPromise(RunCoordinatorOnce);
			const [wait] = await instance.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(ExternalWaits).limit(1),
				),
			);

			expect(cycle.observed_wait_ids).toEqual(["wait_1"]);
			expect(provider_state.calls).toHaveLength(1);
			expect(wait?.observer_lease_owner).toBeNull();
		} finally {
			await instance.dispose();
		}
	});

	it("interrupts a provider read at the lease-safe timeout and records suspension", async () => {
		const started = await Effect.runPromise(Deferred.make<void>());
		const clock = { value: initial_now } satisfies ClockState;
		const provider_state = {
			calls: [],
			lookup: lookup("running"),
			read: () => Deferred.succeed(started, undefined).pipe(Effect.andThen(Effect.never)),
		} satisfies ProviderState;
		const instance = runtime(await Effect.runPromise(MakeDatabasePath), clock, provider_state);

		try {
			await instance.runPromise(SeedWait);
			clock.value = later_now;

			const cycle = instance.runPromise(RunCoordinatorOnce);

			await instance.runPromise(Deferred.await(started));
			await instance.runPromise(TestClock.adjust("20 seconds"));
			await cycle;

			const query = await instance.runPromise(
				Effect.flatMap(ExternalWaitRepository, (external_waits) =>
					external_waits.Query({ thread_id: "thread_1" }),
				),
			);

			expect(query.snapshots[0]?.state).toEqual({
				_tag: "suspended",
				reason: "timeout",
			});
			expect(provider_state.calls).toHaveLength(1);
		} finally {
			await instance.dispose();
		}
	});

	it("suspends project identity drift before contacting the provider", async () => {
		const clock = { value: initial_now } satisfies ClockState;
		const provider_state = {
			calls: [],
			lookup: lookup("running"),
		} satisfies ProviderState;
		const instance = runtime(await Effect.runPromise(MakeDatabasePath), clock, provider_state);

		try {
			await instance.runPromise(SeedWait);
			await instance.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.update(ProjectHostedOrigins).set({ name: "renamed" }),
				),
			);
			clock.value = later_now;

			await instance.runPromise(RunCoordinatorOnce);

			const query = await instance.runPromise(
				Effect.flatMap(ExternalWaitRepository, (external_waits) =>
					external_waits.Query({ thread_id: "thread_1" }),
				),
			);

			expect(query.snapshots[0]?.state).toEqual({
				_tag: "suspended",
				reason: "project_unavailable",
			});
			expect(provider_state.calls).toEqual([]);
		} finally {
			await instance.dispose();
		}
	});

	it("releases the periodic scheduler when its runtime scope closes", async () => {
		const clock = { value: initial_now } satisfies ClockState;
		const scheduler = { active: 0 } satisfies SchedulerState;
		const provider_state = {
			calls: [],
			lookup: lookup("running"),
		} satisfies ProviderState;
		const instance = runtime(await Effect.runPromise(MakeDatabasePath), clock, provider_state, {
			scheduler,
		});

		await instance.runPromise(SeedWait);

		expect(scheduler.active).toBe(1);

		await instance.dispose();

		expect(scheduler.active).toBe(0);
	});
});

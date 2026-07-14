import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Deferred, Effect, Fiber, FileSystem, Layer, ManagedRuntime } from "effect";
import { TestClock } from "effect/testing";
import { afterEach, describe, expect, it } from "vitest";

import { Git } from "../../modules/backend/src/git/git";
import { GitFetch } from "../../modules/backend/src/git/git-fetch";
import { GitMutation } from "../../modules/backend/src/git/git-mutation";
import { WorkspaceGitFetchRepositoryLive } from "../../modules/backend/src/git/workspace-git-fetch-repository";
import {
	WorkspaceGitFetchScheduler,
	WorkspaceGitFetchSchedulerLive,
	WorkspaceGitFetchService,
	WorkspaceGitFetchServiceLive,
} from "../../modules/backend/src/git/workspace-git-fetch-service";
import { make_workspace_git_execution_gate_layer } from "../../modules/backend/src/git/workspace-git-execution-gate";
import {
	WorkspaceGitRegistry,
	type WorkspaceGitCapability,
} from "../../modules/backend/src/git/workspace-git-registry";
import {
	GitTransportAuthentication,
	UnavailableGitTransportAuthenticationLive,
	type GitTransportAuthenticationRequest,
	type GitTransportAuthorization,
} from "../../modules/backend/src/git-provider/git-transport-authentication";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { EventStreams, Threads } from "../../modules/backend/src/persistence/schema";
import {
	ProjectRepository,
	ProjectRepositoryLive,
} from "../../modules/backend/src/projects/project-repository";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-14T15:00:00.000Z";
const root_path = "C:/projects/artisan-editor";
const fetch_result = {
	created_refs: 0,
	deleted_refs: 0,
	remote: "origin" as const,
	remote_refs: 1,
	updated_refs: 1,
};

interface FetchState {
	calls: number;
	gate?: {
		readonly entered: Deferred.Deferred<void>;
		readonly release: Deferred.Deferred<void>;
	};
}

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({ prefix: "artisan-fetch-service-" });

	temporary_directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_capability(state: FetchState): WorkspaceGitCapability {
	const unused_git = {
		DiffPatch: () => Effect.die("unused"),
		DiffStats: Effect.die("unused"),
		Discover: Effect.die("unused"),
		ProbeRepository: Effect.die("unused"),
		ResolveLocalBranch: () => Effect.die("unused"),
		Status: Effect.die("unused"),
		Worktrees: Effect.die("unused"),
	} satisfies typeof Git.Service;
	const unused_mutation = {
		Execute: () => Effect.die("unused"),
		Prepare: () => Effect.die("unused"),
		Reconcile: () => Effect.die("unused"),
	} satisfies typeof GitMutation.Service;

	return {
		canonical_root: root_path,
		fetch: {
			Fetch: () =>
				Effect.gen(function* () {
					state.calls += 1;

					if (state.gate !== undefined) {
						yield* Deferred.succeed(state.gate.entered, undefined);
						yield* Deferred.await(state.gate.release);
					}

					return fetch_result;
				}),
		} satisfies typeof GitFetch.Service,
		mutation: unused_mutation,
		read: unused_git,
		workspace_id: "workspace_fixture",
	};
}

function make_runtime(
	database_path: string,
	instance_id: string,
	state: FetchState,
	options: { readonly now?: string; readonly unavailable_auth?: boolean } = {},
) {
	let next_id = 0;
	const current_time = options.now ?? now;
	const metadata = Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++next_id}`),
		Now: Effect.succeed(current_time),
	});
	const capability = make_capability(state);
	const registry = Layer.succeed(WorkspaceGitRegistry, {
		Get: () => Effect.succeed(capability),
		ListWorkspaceIds: Effect.succeed([]),
	});
	const WithAuthorization: (typeof GitTransportAuthentication.Service)["WithAuthorization"] = <
		A,
		E,
		R,
	>(
		_input: GitTransportAuthenticationRequest,
		use: (authorization: GitTransportAuthorization) => Effect.Effect<A, E, R>,
	) =>
		Effect.scoped(
			use({
				environment: {},
				git_executable_path: "git",
				remote_endpoint: "https://github.com/artisan/editor.git",
				transport_protocol: "https",
			}),
		);
	const authentication = options.unavailable_auth
		? UnavailableGitTransportAuthenticationLive
		: Layer.succeed(GitTransportAuthentication, { WithAuthorization });
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_workspace_git_execution_gate_layer({ database_path }),
		metadata,
		JournalNotifierLive,
		NodeCrypto.layer,
	);
	const projects = ProjectRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const repository = WorkspaceGitFetchRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const service = WorkspaceGitFetchServiceLive.pipe(
		Layer.provideMerge(authentication),
		Layer.provideMerge(projects),
		Layer.provideMerge(repository),
		Layer.provideMerge(registry),
		Layer.provideMerge(
			Layer.succeed(WorkspaceGitFetchScheduler, {
				Schedule: () => Effect.never,
			}),
		),
		Layer.provideMerge(infrastructure),
	);

	return ManagedRuntime.make(service);
}

function request(workspace_id: string, message_id = "fetch_request_1") {
	return {
		message_id,
		sent_at: now,
		thread_id: "thread_1",
		workspace_id,
	};
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
				title: "Fetch test",
				updated_at: now,
			});
			yield* database.client.insert(EventStreams).values({
				last_sequence: 0,
				stream_id: "thread:thread_1",
			});

			return registration.project;
		}),
	);
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

describe("WorkspaceGitFetchService", () => {
	it("runs the hidden production scheduler on one fixed five-minute cadence", async () => {
		let cycles = 0;
		const program = Effect.scoped(
			Effect.gen(function* () {
				const scheduler = yield* WorkspaceGitFetchScheduler;

				yield* Effect.forkScoped(
					scheduler.Schedule(
						Effect.sync(() => {
							cycles += 1;
						}),
					),
				);
				yield* Effect.yieldNow;

				expect(cycles).toBe(0);

				yield* TestClock.adjust("299 seconds");
				yield* Effect.yieldNow;
				expect(cycles).toBe(0);

				yield* TestClock.adjust("1 second");
				yield* Effect.yieldNow;
				expect(cycles).toBe(1);

				yield* TestClock.adjust("5 minutes");
				yield* Effect.yieldNow;
				expect(cycles).toBe(2);
			}),
		).pipe(Effect.provide(WorkspaceGitFetchSchedulerLive), Effect.provide(TestClock.layer()));

		await Effect.runPromise(program);
	});

	it("starts with fetch policy disabled and performs no automatic fetch", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const state: FetchState = { calls: 0 };
		const runtime = make_runtime(database_path, "default_off", state);

		try {
			await seed_project(runtime);
			const query = await runtime.runPromise(
				WorkspaceGitFetchService.pipe(Effect.flatMap((service) => service.Query)),
			);

			expect(query).toEqual({ enabled: false, workspaces: [] });
			expect(state.calls).toBe(0);
		} finally {
			await runtime.dispose();
		}
	});

	it("returns manual acceptance before a deferred background fetch completes", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const state: FetchState = { calls: 0, gate: { entered, release } };
		const runtime = make_runtime(database_path, "manual", state);

		try {
			const project = await seed_project(runtime);
			const acceptance = await runtime.runPromise(
				WorkspaceGitFetchService.pipe(
					Effect.flatMap((service) => service.Request(request(project.workspace_id))),
				),
			);

			expect(acceptance.status).toBe("accepted");
			expect(acceptance.operation.status).toBe("pending");
			await Effect.runPromise(Deferred.await(entered));
			expect(state.calls).toBe(1);

			await Effect.runPromise(Deferred.succeed(release, undefined));
			await runtime.runPromise(
				WorkspaceGitFetchService.pipe(Effect.flatMap((service) => service.AwaitIdle)),
			);
			const query = await runtime.runPromise(
				WorkspaceGitFetchService.pipe(Effect.flatMap((service) => service.Query)),
			);

			expect(query.workspaces).toEqual([
				{
					last_attempt: { attempted_at: now, result: "succeeded" },
					workspace_id: project.workspace_id,
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays an exact concurrent request while executing one fetch", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const state: FetchState = { calls: 0, gate: { entered, release } };
		const runtime = make_runtime(database_path, "duplicate", state);

		try {
			const project = await seed_project(runtime);
			const [first, second] = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* WorkspaceGitFetchService;
					return yield* Effect.all(
						[
							service.Request(request(project.workspace_id)),
							service.Request(request(project.workspace_id)),
						],
						{ concurrency: "unbounded" },
					);
				}),
			);

			expect(first.operation.attempt_id).toBe(second.operation.attempt_id);
			expect(first.event).toEqual(second.event);
			expect([first.status, second.status].sort()).toEqual(["accepted", "duplicate"]);
			await Effect.runPromise(Deferred.await(entered));
			expect(state.calls).toBe(1);
			await Effect.runPromise(Deferred.succeed(release, undefined));
			await runtime.runPromise(
				WorkspaceGitFetchService.pipe(Effect.flatMap((service) => service.AwaitIdle)),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("settles unavailable when transport authentication is unavailable", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const state: FetchState = { calls: 0 };
		const runtime = make_runtime(database_path, "unavailable", state, {
			unavailable_auth: true,
		});

		try {
			const project = await seed_project(runtime);
			await runtime.runPromise(
				WorkspaceGitFetchService.pipe(
					Effect.flatMap((service) => service.Request(request(project.workspace_id))),
				),
			);
			await runtime.runPromise(
				WorkspaceGitFetchService.pipe(Effect.flatMap((service) => service.AwaitIdle)),
			);
			const query = await runtime.runPromise(
				WorkspaceGitFetchService.pipe(Effect.flatMap((service) => service.Query)),
			);

			expect(state.calls).toBe(0);
			expect(query.workspaces).toEqual([
				{
					last_attempt: { attempted_at: now, result: "unavailable" },
					workspace_id: project.workspace_id,
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("waits for admitted manual work before permanently quiescing its thread", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const state: FetchState = { calls: 0, gate: { entered, release } };
		const runtime = make_runtime(database_path, "quiesce", state);

		try {
			const project = await seed_project(runtime);

			await runtime.runPromise(
				WorkspaceGitFetchService.pipe(
					Effect.flatMap((service) => service.Request(request(project.workspace_id))),
				),
			);
			await Effect.runPromise(Deferred.await(entered));

			const quiescence = runtime.runFork(
				WorkspaceGitFetchService.pipe(
					Effect.flatMap((service) => service.QuiesceThread("thread_1")),
				),
			);

			await runtime.runPromise(Effect.yieldNow);
			expect(quiescence.pollUnsafe()).toBeUndefined();

			await Effect.runPromise(Deferred.succeed(release, undefined));
			await runtime.runPromise(Fiber.join(quiescence));
			expect(state.calls).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("takes over one pending attempt after the previous runtime lease expires", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const first_state: FetchState = { calls: 0, gate: { entered, release } };
		const second_state: FetchState = { calls: 0 };
		const first_runtime = make_runtime(database_path, "restart_owner_a", first_state);
		let second_runtime: ManagedRuntime.ManagedRuntime<any, any> | undefined;

		try {
			const project = await seed_project(first_runtime);
			const acceptance = await first_runtime.runPromise(
				WorkspaceGitFetchService.pipe(
					Effect.flatMap((service) => service.Request(request(project.workspace_id))),
				),
			);

			await Effect.runPromise(Deferred.await(entered));
			await first_runtime.dispose();

			second_runtime = make_runtime(database_path, "restart_owner_b", second_state, {
				now: "2026-07-14T15:05:00.000Z",
			});
			await second_runtime.runPromise(
				WorkspaceGitFetchService.pipe(Effect.flatMap((service) => service.AwaitIdle)),
			);
			const query = await second_runtime.runPromise(
				WorkspaceGitFetchService.pipe(Effect.flatMap((service) => service.Query)),
			);

			expect(acceptance.operation.attempt_id).toMatch(/^fetch_restart_owner_a_/u);
			expect(first_state.calls).toBe(1);
			expect(second_state.calls).toBe(1);
			expect(query.workspaces).toEqual([
				{
					last_attempt: {
						attempted_at: "2026-07-14T15:05:00.000Z",
						result: "succeeded",
					},
					workspace_id: project.workspace_id,
				},
			]);
		} finally {
			await first_runtime.dispose();
			await second_runtime?.dispose();
		}
	});

	it("does not let a second runtime execute a live leased fetch", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const first_state: FetchState = { calls: 0, gate: { entered, release } };
		const second_state: FetchState = { calls: 0 };
		const first_runtime = make_runtime(database_path, "lease_owner_a", first_state);
		let second_runtime: ManagedRuntime.ManagedRuntime<any, any> | undefined;

		try {
			const project = await seed_project(first_runtime);
			const acceptance = await first_runtime.runPromise(
				WorkspaceGitFetchService.pipe(
					Effect.flatMap((service) => service.Request(request(project.workspace_id))),
				),
			);
			await Effect.runPromise(Deferred.await(entered));

			second_runtime = make_runtime(database_path, "lease_owner_b", second_state);
			const second_cycle = await second_runtime.runPromise(
				WorkspaceGitFetchService.pipe(Effect.flatMap((service) => service.RunOnce)),
			);

			expect(second_cycle.deferred_attempt_ids).toContain(acceptance.operation.attempt_id);
			expect(second_state.calls).toBe(0);

			await Effect.runPromise(Deferred.succeed(release, undefined));
			await first_runtime.runPromise(
				WorkspaceGitFetchService.pipe(Effect.flatMap((service) => service.AwaitIdle)),
			);
			expect(first_state.calls).toBe(1);
		} finally {
			await first_runtime.dispose();
			await second_runtime?.dispose();
		}
	});
});

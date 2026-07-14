import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Effect, Exit, FileSystem, Layer, ManagedRuntime, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	WorkspaceGitFetchRepository,
	WorkspaceGitFetchRepositoryLive,
	WorkspaceGitFetchUnavailable,
} from "../../modules/backend/src/git/workspace-git-fetch-repository";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	EventStreams,
	JournalEvents,
	Projects,
	Threads,
	WorkspaceChangeOperations,
	WorkspaceMutationAuthorities,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const runtimes: Array<ManagedRuntime.ManagedRuntime<any, any>> = [];
const digest = "a".repeat(64);
const now = "2026-07-14T12:00:00.000Z";
let next_id = 0;

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({ prefix: "artisan-git-fetch-" });

	yield* Effect.sync(() => temporary_directories.push(directory));

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_metadata_layer(instance_id: string) {
	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_fetch_${++next_id}`),
		Now: Effect.sync(() => new Date(Date.parse(now) + next_id++).toISOString()),
	});
}

function make_runtime(database_path: string, instance_id: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(instance_id),
		JournalNotifierLive,
	);

	const runtime = ManagedRuntime.make(
		WorkspaceGitFetchRepositoryLive.pipe(Layer.provideMerge(infrastructure)),
	);

	runtimes.push(runtime);

	return runtime;
}

async function make_database_path() {
	return Effect.runPromise(MakeDatabasePath);
}

function Fetch<A>(
	effect: (repository: WorkspaceGitFetchRepository["Service"]) => Effect.Effect<A, unknown>,
) {
	return Effect.flatMap(WorkspaceGitFetchRepository, effect);
}

async function run<A>(
	runtime: ManagedRuntime.ManagedRuntime<any, any>,
	effect: Effect.Effect<A, any, any>,
) {
	return runtime.runPromise(effect);
}

async function seed_thread(runtime: ManagedRuntime.ManagedRuntime<any, any>) {
	await run(
		runtime,
		Effect.gen(function* () {
			const database = yield* Database;
			const project = {
				display_name: "Artisan",
				project_id: "project_1",
				root_path: "C:/artisan",
			};

			yield* database.client.insert(Threads).values({
				created_at: now,
				linked_projects_json: "[]",
				primary_project_id: project.project_id,
				primary_project_json: JSON.stringify(project),
				thread_id: "thread_1",
				title: "Fetch",
				title_source: "initial",
				updated_at: now,
			});
			yield* database.client.insert(Projects).values({
				canonical_root: project.root_path,
				display_name: project.display_name,
				project_id: project.project_id,
				registered_at: now,
				updated_at: now,
				workspace_id: "workspace_1",
			});
			yield* database.client
				.insert(EventStreams)
				.values({ last_sequence: 0, stream_id: "thread:thread_1" });
		}),
	);
}

function policy_input(message_id = "policy_1", enabled = true) {
	return { enabled, message_id, request_fingerprint: digest, sent_at: now };
}

function manual_input(message_id = "manual_1", attempt_id = "attempt_1") {
	return {
		attempt_id,
		message_id,
		request_fingerprint: digest,
		sent_at: now,
		thread_id: "thread_1",
		workspace_id: "workspace_1",
	};
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected failure");
}

afterEach(async () => {
	const directories = temporary_directories.splice(0);
	const active_runtimes = runtimes.splice(0);

	await Promise.all(active_runtimes.map((runtime) => runtime.dispose()));

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

describe("WorkspaceGitFetchRepository", () => {
	it("defaults off with an empty durable projection", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path, "fetch_default");

		expect(
			await run(
				runtime,
				Fetch((repository) => repository.Query),
			),
		).toEqual({ enabled: false, workspaces: [] });
	});

	it("journals one policy receipt and replays it exactly", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path, "fetch_policy");
		const accepted = await run(
			runtime,
			Fetch((repository) => repository.UpdatePolicy(policy_input())),
		);
		const duplicate = await run(
			runtime,
			Fetch((repository) => repository.UpdatePolicy(policy_input())),
		);

		expect(accepted.status).toBe("accepted");
		expect(duplicate).toMatchObject({ event: accepted.event, status: "duplicate" });

		const changed = await runtime.runPromiseExit(
			Fetch((repository) => repository.UpdatePolicy(policy_input("policy_1", false))),
		);
		expect(failure_from(changed)).toMatchObject({ reason: "request_conflict" });
	});

	it("requires a live attached thread for manual work", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path, "fetch_attachment");

		const missing = await runtime.runPromiseExit(
			Fetch((repository) => repository.PrepareManual(manual_input())),
		);
		expect(failure_from(missing)).toBeInstanceOf(WorkspaceGitFetchUnavailable);

		await seed_thread(runtime);
		const accepted = await run(
			runtime,
			Fetch((repository) => repository.PrepareManual(manual_input())),
		);
		expect(accepted).toMatchObject({ operation: { status: "pending" }, status: "accepted" });
	});

	it("replays one manual intent across independently generated attempt identifiers", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path, "fetch_manual_replay");

		await seed_thread(runtime);
		const accepted = await run(
			runtime,
			Fetch((repository) => repository.PrepareManual(manual_input())),
		);
		const duplicate = await run(
			runtime,
			Fetch((repository) => repository.PrepareManual(manual_input("manual_1", "attempt_2"))),
		);

		expect(accepted.operation.attempt_id).toBe("attempt_1");
		expect(duplicate).toMatchObject({
			event: accepted.event,
			operation: { attempt_id: "attempt_1" },
			status: "duplicate",
		});
	});

	it("leases manual work once, recovers it after expiry, and leaves release pending", async () => {
		const database_path = await make_database_path();
		const first = make_runtime(database_path, "fetch_one");
		const second = make_runtime(database_path, "fetch_two");

		await seed_thread(first);
		await run(
			first,
			Fetch((repository) => repository.PrepareManual(manual_input())),
		);
		const first_claim = await run(
			first,
			Fetch((repository) =>
				repository.ClaimManual({
					lease_expires_at: "2026-07-14T12:01:00.000Z",
					lease_owner: "owner_1",
					message_id: "manual_1",
					now,
				}),
			),
		);
		const contended_claim = await run(
			second,
			Fetch((repository) =>
				repository.ClaimManual({
					lease_expires_at: "2026-07-14T12:02:00.000Z",
					lease_owner: "owner_2",
					message_id: "manual_1",
					now,
				}),
			),
		);
		const recovered_claim = await run(
			second,
			Fetch((repository) =>
				repository.ClaimManual({
					lease_expires_at: "2026-07-14T12:03:00.000Z",
					lease_owner: "owner_2",
					message_id: "manual_1",
					now: "2026-07-14T12:01:00.000Z",
				}),
			),
		);

		expect(Option.isSome(first_claim)).toBe(true);
		expect(Option.isNone(contended_claim)).toBe(true);
		expect(Option.isSome(recovered_claim)).toBe(true);
		await run(
			second,
			Fetch((repository) =>
				repository.ReleaseClaim({
					attempt_id: "attempt_1",
					lease_owner: "owner_2",
					workspace_id: "workspace_1",
				}),
			),
		);

		expect(
			await run(
				second,
				Fetch((repository) => repository.ReadManual("manual_1")),
			),
		).toMatchObject({ value: { status: "pending" } });
	});

	it("defers a claim and verification while a controlled writer owns the workspace", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path, "fetch_writer");

		await seed_thread(runtime);
		await run(
			runtime,
			Fetch((repository) => repository.PrepareManual(manual_input())),
		);
		await run(
			runtime,
			Fetch((repository) =>
				repository.ClaimManual({
					lease_expires_at: "2026-07-14T12:01:00.000Z",
					lease_owner: "owner",
					message_id: "manual_1",
					now,
				}),
			),
		);
		await run(
			runtime,
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(WorkspaceChangeOperations).values({
					action: "replace",
					change_id: "change_1",
					created_at: now,
					lifecycle: "claimed",
					message_id: "writer_1",
					request_fingerprint: digest,
					sent_at: now,
					thread_id: "thread_1",
					updated_at: now,
					workspace_id: "workspace_1",
				});
				yield* database.client.insert(WorkspaceMutationAuthorities).values({
					agent_id: "agent_1",
					authority_kind: "base_run",
					change_id: "change_1",
					created_at: now,
					message_id: "writer_1",
					run_id: "run_1",
					thread_id: "thread_1",
					workspace_id: "workspace_1",
					working_directory: "C:/artisan",
				});
			}),
		);

		expect(
			Option.isNone(
				await run(
					runtime,
					Fetch((repository) =>
						repository.VerifyClaim({
							attempt_id: "attempt_1",
							lease_expires_at: "2026-07-14T12:04:00.000Z",
							lease_owner: "owner",
							now: "2026-07-14T12:00:30.000Z",
							workspace_id: "workspace_1",
						}),
					),
				),
			),
		).toBe(true);
	});

	it("renews a live claim at verification and recovers an expired automatic identity", async () => {
		const database_path = await make_database_path();
		const first = make_runtime(database_path, "fetch_lease_first");
		const second = make_runtime(database_path, "fetch_lease_second");

		await seed_thread(first);
		await run(
			first,
			Fetch((repository) => repository.PrepareManual(manual_input())),
		);
		await run(
			first,
			Fetch((repository) =>
				repository.ClaimManual({
					lease_expires_at: "2026-07-14T12:01:00.000Z",
					lease_owner: "owner_1",
					message_id: "manual_1",
					now,
				}),
			),
		);
		const verified = await run(
			first,
			Fetch((repository) =>
				repository.VerifyClaim({
					attempt_id: "attempt_1",
					lease_expires_at: "2026-07-14T12:04:00.000Z",
					lease_owner: "owner_1",
					now: "2026-07-14T12:00:30.000Z",
					workspace_id: "workspace_1",
				}),
			),
		);
		const premature_takeover = await run(
			second,
			Fetch((repository) =>
				repository.ClaimManual({
					lease_expires_at: "2026-07-14T12:05:00.000Z",
					lease_owner: "owner_2",
					message_id: "manual_1",
					now: "2026-07-14T12:01:00.000Z",
				}),
			),
		);

		expect(Option.isSome(verified)).toBe(true);
		expect(Option.isNone(premature_takeover)).toBe(true);

		await run(
			first,
			Fetch((repository) =>
				repository.CompleteClaim({
					attempt_id: "attempt_1",
					attempted_at: "2026-07-14T12:01:30.000Z",
					lease_owner: "owner_1",
					result: "succeeded",
					workspace_id: "workspace_1",
				}),
			),
		);
		await run(
			first,
			Fetch((repository) => repository.UpdatePolicy(policy_input("policy_lease"))),
		);
		const first_automatic = await run(
			first,
			Fetch((repository) =>
				repository.ClaimAutomatic({
					attempt_id: "auto_1",
					due_before: "2026-07-14T12:02:00.000Z",
					lease_expires_at: "2026-07-14T12:03:00.000Z",
					lease_owner: "owner_1",
					now: "2026-07-14T12:02:00.000Z",
					workspace_id: "workspace_1",
				}),
			),
		);
		const recovered_automatic = await run(
			second,
			Fetch((repository) =>
				repository.ClaimAutomatic({
					attempt_id: "auto_2",
					due_before: "2026-07-14T12:03:00.000Z",
					lease_expires_at: "2026-07-14T12:07:00.000Z",
					lease_owner: "owner_2",
					now: "2026-07-14T12:03:00.000Z",
					workspace_id: "workspace_1",
				}),
			),
		);

		expect(first_automatic).toMatchObject({ value: { attempt_id: "auto_1" } });
		expect(recovered_automatic).toMatchObject({ value: { attempt_id: "auto_1" } });
	});

	it("keeps automatic fetch off by default, prioritizes manual work, and honors due time", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path, "fetch_due");

		await seed_thread(runtime);
		const automatic = {
			attempt_id: "auto_1",
			due_before: "2026-07-14T12:01:00.000Z",
			lease_expires_at: "2026-07-14T12:02:00.000Z",
			lease_owner: "auto",
			now: "2026-07-14T12:01:00.000Z",
			workspace_id: "workspace_1",
		};

		expect(
			Option.isNone(
				await run(
					runtime,
					Fetch((repository) => repository.ClaimAutomatic(automatic)),
				),
			),
		).toBe(true);
		await run(
			runtime,
			Fetch((repository) => repository.UpdatePolicy(policy_input("policy_due"))),
		);
		await run(
			runtime,
			Fetch((repository) =>
				repository.PrepareManual(manual_input("manual_due", "attempt_due")),
			),
		);
		expect(
			Option.isNone(
				await run(
					runtime,
					Fetch((repository) => repository.ClaimAutomatic(automatic)),
				),
			),
		).toBe(true);
		await run(
			runtime,
			Fetch((repository) =>
				repository.ClaimManual({
					lease_expires_at: "2026-07-14T12:02:00.000Z",
					lease_owner: "manual",
					message_id: "manual_due",
					now: "2026-07-14T12:01:00.000Z",
				}),
			),
		);
		await run(
			runtime,
			Fetch((repository) =>
				repository.CompleteClaim({
					attempt_id: "attempt_due",
					attempted_at: "2026-07-14T12:01:30.000Z",
					lease_owner: "manual",
					result: "succeeded",
					workspace_id: "workspace_1",
				}),
			),
		);
		expect(
			Option.isNone(
				await run(
					runtime,
					Fetch((repository) => repository.ClaimAutomatic(automatic)),
				),
			),
		).toBe(true);
		expect(
			Option.isSome(
				await run(
					runtime,
					Fetch((repository) =>
						repository.ClaimAutomatic({
							...automatic,
							due_before: "2026-07-14T12:02:00.000Z",
						}),
					),
				),
			),
		).toBe(true);
	});

	it("journals manual completion once, retains compact state, and leaves automatic completion silent", async () => {
		const database_path = await make_database_path();
		const first = make_runtime(database_path, "fetch_complete");

		await seed_thread(first);
		const accepted = await run(
			first,
			Fetch((repository) => repository.PrepareManual(manual_input())),
		);
		await run(
			first,
			Fetch((repository) =>
				repository.ClaimManual({
					lease_expires_at: "2026-07-14T12:01:00.000Z",
					lease_owner: "owner",
					message_id: "manual_1",
					now,
				}),
			),
		);
		await run(
			first,
			Fetch((repository) =>
				repository.CompleteClaim({
					attempt_id: "attempt_1",
					attempted_at: "2026-07-14T12:00:30.000Z",
					lease_owner: "owner",
					result: "succeeded",
					workspace_id: "workspace_1",
				}),
			),
		);

		const restarted = make_runtime(database_path, "fetch_restart");
		expect(
			await run(
				restarted,
				Fetch((repository) => repository.PrepareManual(manual_input())),
			),
		).toMatchObject({
			event: accepted.event,
			operation: { result: "succeeded", status: "terminal" },
			status: "duplicate",
		});

		await run(
			restarted,
			Fetch((repository) => repository.UpdatePolicy(policy_input("policy_2"))),
		);
		await run(
			restarted,
			Fetch((repository) =>
				repository.ClaimAutomatic({
					attempt_id: "auto_1",
					due_before: "2026-07-14T12:01:00.000Z",
					lease_expires_at: "2026-07-14T12:02:00.000Z",
					lease_owner: "auto",
					now: "2026-07-14T12:01:00.000Z",
					workspace_id: "workspace_1",
				}),
			),
		);
		await run(
			restarted,
			Fetch((repository) =>
				repository.CompleteClaim({
					attempt_id: "auto_1",
					attempted_at: "2026-07-14T12:01:10.000Z",
					lease_owner: "auto",
					result: "failed",
					workspace_id: "workspace_1",
				}),
			),
		);

		const events = await run(
			restarted,
			Effect.gen(function* () {
				const database = yield* Database;
				return yield* database.client.select().from(JournalEvents);
			}),
		);
		expect(
			events.filter((event) => event.event_type === "workspace.git.fetch.policy.updated"),
		).toHaveLength(1);
		expect(
			events.filter((event) => event.event_type === "workspace.git.fetch.requested"),
		).toHaveLength(1);
		expect(
			events.filter((event) => event.event_type === "workspace.git.fetch.completed"),
		).toHaveLength(1);
		expect(
			await run(
				restarted,
				Fetch((repository) => repository.Query),
			),
		).toMatchObject({
			workspaces: [{ last_attempt: { result: "failed" }, workspace_id: "workspace_1" }],
		});
	});
});

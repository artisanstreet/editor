import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Effect, Exit, FileSystem, Layer, ManagedRuntime, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	HostedProjectCloneConflict,
	HostedProjectCloneInvariant,
	HostedProjectCloneRepository,
	HostedProjectCloneRepositoryLive,
	HostedProjectCloneUnavailable,
} from "../../modules/backend/src/projects/hosted-project-clone-repository";
import { make_workspace_git_execution_gate_layer } from "../../modules/backend/src/git/workspace-git-execution-gate";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	HostedProjectCloneArtifacts,
	HostedProjectCloneClaims,
	JournalCommands,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const digest = "a".repeat(64);

interface MutableClock {
	value: string;
}

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({ prefix: "artisan-hosted-clone-" });

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(database_path: string, instance_id: string, clock: MutableClock) {
	let next_id = 0;

	const metadata = Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++next_id}`),
		Now: Effect.sync(() => clock.value),
	});
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_workspace_git_execution_gate_layer({ database_path }),
		metadata,
		JournalNotifierLive,
	);

	return ManagedRuntime.make(
		HostedProjectCloneRepositoryLive.pipe(Layer.provideMerge(infrastructure)),
	);
}

function source_command(message_id = "clone_request_1") {
	return { message_id, sent_at: "2026-07-14T12:00:00.000Z" };
}

function repository(native_id = "repository_1") {
	return {
		archived: false,
		clone_url: "https://github.com/artisan/editor.git",
		default_branch: { _tag: "known" as const, name: "main" },
		identity: { host: "github.com", name: "editor", owner: "artisan", provider_id: "github" },
		origin: { native_id, provider_id: "github", resource_kind: "repository" as const },
		viewer_permission: "write" as const,
		visibility: "private" as const,
		web_url: "https://github.com/artisan/editor",
	};
}

function clone_request(
	overrides: {
		readonly approval_id?: string;
		readonly destination_path?: string;
		readonly native_id?: string;
		readonly source_command_id?: string;
		readonly thread_id?: string;
	} = {},
) {
	const selected_repository = repository(overrides.native_id);
	const destination_path = overrides.destination_path ?? "C:/projects/editor";

	return {
		approval_id: overrides.approval_id ?? "approval_1",
		destination: {
			canonical_root: destination_path,
			projects_root: "C:/projects",
			projects_root_device: "1",
			projects_root_inode: "2",
			root_device: "1",
			root_inode: "3",
		},
		preparation: {
			repository: selected_repository,
			selection: { account_login: "artisan", host: "github.com", provider_id: "github" },
		},
		request: {
			repository: selected_repository,
			selection: { account_login: "artisan", host: "github.com", provider_id: "github" },
		},
		request_fingerprint: digest,
		source_command: source_command(overrides.source_command_id),
		thread_id: overrides.thread_id ?? "thread_1",
	};
}

function reused_request() {
	const request = clone_request();

	return {
		approval_id: request.approval_id,
		attachment: "already_attached" as const,
		destination_path: request.destination.canonical_root,
		registered_project: registered_project(),
		request: request.request,
		request_fingerprint: request.request_fingerprint,
		source_command: request.source_command,
		thread_id: request.thread_id,
	};
}

function decision(
	approved: boolean,
	message_id = approved ? "clone_approved_1" : "clone_denied_1",
) {
	return {
		approval_id: "approval_1",
		approved,
		decision_command: source_command(message_id),
		thread_id: "thread_1",
	};
}

function clone_result() {
	return {
		canonical_root: "C:/projects/editor",
		output_complete: true,
		repository: repository(),
		type: "cloned" as const,
	};
}

function project_ref() {
	return {
		display_name: "Artisan Editor",
		project_id: "project_" + "b".repeat(64),
		root_path: "C:/projects/editor",
	};
}

function registered_project() {
	return {
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
			selected_account_login: "artisan",
			web_url: "https://github.com/artisan/editor",
		},
		project: project_ref(),
		registered_at: "2026-07-14T12:00:00.000Z",
		updated_at: "2026-07-14T12:00:00.000Z",
		workspace_id: "workspace_" + "c".repeat(64),
	};
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
});

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected the Effect to fail");
}

function expect_conflict(exit: Exit.Exit<unknown, unknown>, reason: string) {
	const failure = failure_from(exit);

	expect(failure).toBeInstanceOf(HostedProjectCloneConflict);
	expect(failure).toMatchObject({ reason });
}

afterEach(async () => {
	const cleanup = directories.splice(0);

	await Effect.runPromise(
		Effect.forEach(
			cleanup,
			(directory) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(directory, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("HostedProjectCloneRepository", () => {
	it("redacts source data from journal records while replaying only the exact request", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "redaction", {
			value: "2026-07-14T12:00:00.000Z",
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const clones = yield* HostedProjectCloneRepository;

					yield* SeedThread;
					const accepted = yield* clones.Request(clone_request());
					const replay = yield* clones.Request(clone_request());
					const conflict = yield* clones
						.Request(clone_request({ destination_path: "C:/projects/other" }))
						.pipe(Effect.exit);

					return {
						accepted,
						commands: yield* database.client.select().from(JournalCommands),
						conflict,
						events: yield* database.client.select().from(JournalEvents),
						replay,
					};
				}),
			);

			expect(result.accepted.status).toBe("accepted");
			expect(result.replay).toEqual({ ...result.accepted, status: "duplicate" });
			expect_conflict(result.conflict, "request_conflict");
			expect(JSON.stringify([result.commands, result.events])).not.toContain("repository_1");
			expect(JSON.stringify([result.commands, result.events])).not.toContain(
				"https://github.com/artisan/editor.git",
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("records direct reuse without a private artifact or active clone claim", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "reuse", { value: "2026-07-14T12:00:00.000Z" });

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const clones = yield* HostedProjectCloneRepository;

					yield* SeedThread;
					const accepted = yield* clones.RecordReused(reused_request());
					const replay = yield* clones.RecordReused(reused_request());

					return {
						artifacts: yield* database.client
							.select()
							.from(HostedProjectCloneArtifacts),
						claims: yield* database.client.select().from(HostedProjectCloneClaims),
						accepted,
						replay,
					};
				}),
			);

			expect(result.accepted.approval).toMatchObject({ state: "reused" });
			expect(result.replay).toEqual({ ...result.accepted, status: "duplicate" });
			expect(result.artifacts).toEqual([]);
			expect(result.claims).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays one decision exactly and releases the reservation when denied", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "deny", { value: "2026-07-14T12:00:00.000Z" });

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const clones = yield* HostedProjectCloneRepository;

					yield* SeedThread;
					yield* clones.Request(clone_request());
					const denied = yield* clones.Decide(decision(false));
					const replay = yield* clones.Decide(decision(false));
					const conflict = yield* clones
						.Decide(decision(true, "clone_denied_1"))
						.pipe(Effect.exit);

					return {
						claims: yield* database.client.select().from(HostedProjectCloneClaims),
						conflict,
						denied,
						replay,
					};
				}),
			);

			expect(result.denied.approval.state).toBe("denied");
			expect(result.replay).toEqual({ ...result.denied, status: "duplicate" });
			expect_conflict(result.conflict, "decision_conflict");
			expect(result.claims).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("persists the marked, executed, cloned, registered, and applied lifecycle", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "lifecycle", {
			value: "2026-07-14T12:00:00.000Z",
		});
		let executions = 0;

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const clones = yield* HostedProjectCloneRepository;

					yield* SeedThread;
					yield* clones.Request(clone_request());
					yield* clones.Decide(decision(true));
					yield* clones.MarkExecuting("approval_1");
					const execution = yield* clones.ReadExecution("approval_1");
					const identity = {
						approval_id: "approval_1",
						claim_token: execution.claim_token,
					};

					yield* clones.ExecuteClaimed(
						identity,
						Effect.sync(() => ++executions),
					);
					yield* clones.RecordCloneResult({ ...identity, result: clone_result() });
					yield* clones.RecordRegisteredProject({
						...identity,
						project: registered_project(),
					});
					const applied = yield* clones.Settle({
						...identity,
						attachment: "attached",
						project: project_ref(),
						type: "applied",
					});

					return {
						artifacts: yield* database.client
							.select()
							.from(HostedProjectCloneArtifacts),
						claims: yield* database.client.select().from(HostedProjectCloneClaims),
						applied,
					};
				}),
			);

			expect(executions).toBe(1);
			expect(result.applied.approval).toMatchObject({
				attachment: "attached",
				state: "applied",
			});
			expect(result.claims).toEqual([]);
			expect(result.artifacts[0]).toMatchObject({
				clone_result_json: expect.stringContaining('"type":"cloned"'),
				registered_project_json: expect.stringContaining('"project_id"'),
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("uses the durable external marker to reject a second clone execution", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "marker", {
			value: "2026-07-14T12:00:00.000Z",
		});
		let executions = 0;

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const clones = yield* HostedProjectCloneRepository;

					yield* SeedThread;
					yield* clones.Request(clone_request());
					yield* clones.Decide(decision(true));
					yield* clones.MarkExecuting("approval_1");
					const execution = yield* clones.ReadExecution("approval_1");
					const identity = {
						approval_id: "approval_1",
						claim_token: execution.claim_token,
					};

					yield* clones.ExecuteClaimed(
						identity,
						Effect.sync(() => ++executions),
					);
					const replay = yield* clones
						.ExecuteClaimed(
							identity,
							Effect.sync(() => ++executions),
						)
						.pipe(Effect.exit);

					return replay;
				}),
			);

			expect(executions).toBe(1);
			expect_conflict(result, "lease_conflict");
		} finally {
			await runtime.dispose();
		}
	});

	it("holds destination and hosted-identity claims across concurrent requests", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const clock = { value: "2026-07-14T12:00:00.000Z" };
		const first_runtime = make_runtime(database_path, "claim_a", clock);
		const second_runtime = make_runtime(database_path, "claim_b", clock);

		try {
			await first_runtime.runPromise(SeedThread);
			await first_runtime.runPromise(
				Effect.flatMap(HostedProjectCloneRepository, (clones) =>
					clones.Request(clone_request()),
				),
			);
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					const clones = yield* HostedProjectCloneRepository;
					const destination = yield* clones
						.Request(
							clone_request({
								approval_id: "approval_2",
								source_command_id: "clone_request_2",
							}),
						)
						.pipe(Effect.exit);
					const identity = yield* clones
						.Request(
							clone_request({
								approval_id: "approval_3",
								destination_path: "C:/projects/other",
								source_command_id: "clone_request_3",
							}),
						)
						.pipe(Effect.exit);

					return { destination, identity };
				}),
			);

			expect_conflict(result.destination, "claim_conflict");
			expect_conflict(result.identity, "claim_conflict");
		} finally {
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	});

	it("recovers an expired approval before provider execution starts", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const clock = { value: "2026-07-14T12:00:00.000Z" };
		const first_runtime = make_runtime(database_path, "before_start", clock);

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const clones = yield* HostedProjectCloneRepository;

					yield* SeedThread;
					yield* clones.Request(clone_request());
					yield* clones.Decide(decision(true));
					yield* clones.MarkExecuting("approval_1");
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		clock.value = "2026-07-14T12:01:00.000Z";
		const restarted = make_runtime(database_path, "after_restart", clock);

		try {
			const recovered = await restarted.runPromise(
				Effect.flatMap(HostedProjectCloneRepository, (clones) =>
					clones.ClaimRecovery("approval_1"),
				),
			);

			expect(Option.isSome(recovered)).toBe(true);
			expect(Option.getOrThrow(recovered).clone_result).toBeUndefined();
		} finally {
			await restarted.dispose();
		}
	});

	it("recovers a completed clone result without re-running the external clone", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const clock = { value: "2026-07-14T12:00:00.000Z" };
		const first_runtime = make_runtime(database_path, "cloned", clock);

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const clones = yield* HostedProjectCloneRepository;

					yield* SeedThread;
					yield* clones.Request(clone_request());
					yield* clones.Decide(decision(true));
					yield* clones.MarkExecuting("approval_1");
					const execution = yield* clones.ReadExecution("approval_1");
					const identity = {
						approval_id: "approval_1",
						claim_token: execution.claim_token,
					};

					yield* clones.ExecuteClaimed(identity, Effect.void);
					yield* clones.RecordCloneResult({ ...identity, result: clone_result() });
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		clock.value = "2026-07-14T12:01:00.000Z";
		const restarted = make_runtime(database_path, "result_recovery", clock);
		let external_executions = 0;

		try {
			const result = await restarted.runPromise(
				Effect.gen(function* () {
					const clones = yield* HostedProjectCloneRepository;
					const recovered = Option.getOrThrow(yield* clones.ClaimRecovery("approval_1"));
					const replay = yield* clones
						.ExecuteClaimed(
							{ approval_id: "approval_1", claim_token: recovered.claim_token },
							Effect.sync(() => ++external_executions),
						)
						.pipe(Effect.exit);

					return { recovered, replay };
				}),
			);

			expect(result.recovered.clone_result).toEqual(clone_result());
			expect(external_executions).toBe(0);
			expect_conflict(result.replay, "lease_conflict");
		} finally {
			await restarted.dispose();
		}
	});

	it("quarantines an expired started clone without result while retaining its claim", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const clock = { value: "2026-07-14T12:00:00.000Z" };
		const first_runtime = make_runtime(database_path, "started", clock);

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const clones = yield* HostedProjectCloneRepository;

					yield* SeedThread;
					yield* clones.Request(clone_request());
					yield* clones.Decide(decision(true));
					yield* clones.MarkExecuting("approval_1");
					const execution = yield* clones.ReadExecution("approval_1");

					yield* clones.ExecuteClaimed(
						{ approval_id: "approval_1", claim_token: execution.claim_token },
						Effect.void,
					);
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		clock.value = "2026-07-14T12:01:00.000Z";
		const restarted = make_runtime(database_path, "quarantine", clock);

		try {
			const result = await restarted.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const clones = yield* HostedProjectCloneRepository;
					const dispatches = yield* clones.ListExecuting;
					const quarantined = yield* clones.QuarantineInterrupted("approval_1");

					return {
						claims: yield* database.client.select().from(HostedProjectCloneClaims),
						dispatches,
						quarantined,
					};
				}),
			);

			expect(result.dispatches).toEqual([
				{ approval_id: "approval_1", recovery: "quarantine", thread_id: "thread_1" },
			]);
			expect(result.quarantined.approval).toMatchObject({
				reason: "interrupted",
				state: "outcome_unknown",
			});
			expect(result.claims).toHaveLength(1);
		} finally {
			await restarted.dispose();
		}
	});

	it("fails closed on corrupted private state and an erasing thread", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "fence", { value: "2026-07-14T12:00:00.000Z" });

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const clones = yield* HostedProjectCloneRepository;

					yield* SeedThread;
					yield* clones.Request(clone_request());
					yield* database.client
						.update(HostedProjectCloneArtifacts)
						.set({ request_json: "{not json" });
					const corrupt = yield* clones
						.Query({ approval_id: "approval_1", thread_id: "thread_1" })
						.pipe(Effect.exit);

					yield* database.client
						.update(HostedProjectCloneArtifacts)
						.set({ request_json: JSON.stringify(clone_request().request) });
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-14T12:00:00.000Z",
						thread_id: "thread_1",
					});
					const erased = yield* clones
						.Query({ approval_id: "approval_1", thread_id: "thread_1" })
						.pipe(Effect.exit);

					return { corrupt, erased };
				}),
			);

			expect(failure_from(result.corrupt)).toBeInstanceOf(HostedProjectCloneInvariant);
			expect(failure_from(result.erased)).toEqual(
				new HostedProjectCloneUnavailable({ reason: "erased" }),
			);
		} finally {
			await runtime.dispose();
		}
	});
});

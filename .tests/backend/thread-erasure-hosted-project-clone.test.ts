import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_workspace_git_execution_gate_layer } from "../../modules/backend/src/git/workspace-git-execution-gate";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	EventStreams,
	HostedProjectCloneApprovals,
	HostedProjectCloneArtifacts,
	HostedProjectCloneClaims,
	ProjectHostedOrigins,
	Projects,
	ThreadErasureClaims,
	ThreadProjectAffinityEvidence,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import {
	HostedProjectCloneRepository,
	HostedProjectCloneRepositoryLive,
} from "../../modules/backend/src/projects/hosted-project-clone-repository";
import {
	ProjectRepository,
	ProjectRepositoryLive,
} from "../../modules/backend/src/projects/project-repository";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { ThreadErasure, ThreadErasureLive } from "../../modules/backend/src/threads/thread-erasure";
import { ThreadResourceQuiescer } from "../../modules/backend/src/threads/thread-resource-quiescer";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const created_at = "2026-07-14T12:00:00.000Z";
const expired_at = "2026-07-14T12:01:00.000Z";
const cutoff = "2026-07-19T12:00:00.000Z";
const deleted_at = "2026-07-20T12:00:00.000Z";
const digest = "a".repeat(64);

interface MutableClock {
	value: string;
}

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-thread-erasure-hosted-clone-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

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

function clone_request(thread_id: string, approval_id: string) {
	const selected_repository = repository();

	return {
		approval_id,
		destination: {
			canonical_root: "C:/projects/editor",
			projects_root: "C:/projects",
			projects_root_device: "1",
			projects_root_inode: "2",
			root_device: "1",
			root_inode: "3",
		},
		preparation: {
			repository: selected_repository,
			selection: { account_login: "sander", host: "github.com", provider_id: "github" },
		},
		request: {
			repository: selected_repository,
			selection: { account_login: "sander", host: "github.com", provider_id: "github" },
		},
		request_fingerprint: digest,
		source_command: { message_id: `${approval_id}_request`, sent_at: created_at },
		thread_id,
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

function project_registration() {
	return {
		canonical_root: "C:/projects/editor",
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
			remote_name: "origin" as const,
			selected_account_login: "sander",
			web_url: "https://github.com/artisan/editor",
		},
	};
}

function make_runtime(database_path: string, clock: MutableClock) {
	let next_id = 0;

	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_workspace_git_execution_gate_layer({ database_path }),
		Layer.succeed(RuntimeMetadata, {
			instance_id: "thread_erasure_hosted_clone_test",
			MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
			Now: Effect.sync(() => clock.value),
		}),
		Layer.succeed(ThreadResourceQuiescer, { Quiesce: () => Effect.void }),
		JournalNotifierLive,
		NodeCrypto.layer,
	);
	const repositories = Layer.mergeAll(
		HostedProjectCloneRepositoryLive,
		ProjectRepositoryLive,
	).pipe(Layer.provideMerge(infrastructure));
	const erasure = ThreadErasureLive.pipe(Layer.provideMerge(infrastructure));

	return ManagedRuntime.make(Layer.merge(repositories, erasure));
}

const SeedThread = (thread_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at,
			last_activity_at: created_at,
			thread_id,
			title: thread_id,
			title_source: "initial",
			updated_at: created_at,
		});
		yield* database.client.insert(EventStreams).values({
			last_sequence: 0,
			stream_id: `thread:${thread_id}`,
		});
	});

const PrepareCloneState = (
	state: "requested" | "approved" | "executing" | "outcome_unknown",
	thread_id: string,
	clock: MutableClock,
) =>
	Effect.gen(function* () {
		const clones = yield* HostedProjectCloneRepository;
		const approval_id = `approval_${state}`;

		yield* clones.Request(clone_request(thread_id, approval_id));

		if (state === "requested") {
			return;
		}

		yield* clones.Decide({
			approval_id,
			approved: true,
			decision_command: { message_id: `${approval_id}_decision`, sent_at: created_at },
			thread_id,
		});

		if (state === "approved") {
			return;
		}

		yield* clones.MarkExecuting(approval_id);

		if (state === "executing") {
			return;
		}

		const execution = yield* clones.ReadExecution(approval_id);

		yield* clones.ExecuteClaimed(
			{ approval_id, claim_token: execution.claim_token },
			Effect.void,
		);
		clock.value = expired_at;
		yield* clones.QuarantineInterrupted(approval_id);
	});

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

describe("ThreadErasure hosted project clone state", () => {
	it.each(["requested", "approved", "executing", "outcome_unknown"] as const)(
		"fences %s clone state from automatic erasure",
		async (state) => {
			const clock = { value: created_at };
			const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath), clock);
			const thread_id = `thread_${state}`;

			try {
				const result = await runtime.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const erasure = yield* ThreadErasure;

						yield* SeedThread(thread_id);
						yield* PrepareCloneState(state, thread_id, clock);
						const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

						return {
							approvals: yield* database.client
								.select()
								.from(HostedProjectCloneApprovals),
							claims: yield* database.client.select().from(HostedProjectCloneClaims),
							erased,
							erasure_claims: yield* database.client
								.select()
								.from(ThreadErasureClaims),
							threads: yield* database.client.select().from(Threads),
						};
					}),
				);

				expect(result.erased).toEqual([]);
				expect(result.erasure_claims).toEqual([]);
				expect(result.threads.map((thread) => thread.thread_id)).toEqual([thread_id]);
				expect(result.approvals[0]?.state).toBe(state);
				expect(result.claims).toHaveLength(1);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it("drops an erasure claim when a clone becomes pending before deletion", async () => {
		const clock = { value: created_at };
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath), clock);
		const thread_id = "thread_claim_race";

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const clones = yield* HostedProjectCloneRepository;
					const database = yield* Database;
					const erasure = yield* ThreadErasure;

					yield* SeedThread(thread_id);
					yield* clones.Request(clone_request(thread_id, "approval_claim_race"));
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: deleted_at,
						thread_id,
					});

					const erased = yield* erasure.ResumeClaimed(deleted_at);

					return {
						approvals: yield* database.client
							.select()
							.from(HostedProjectCloneApprovals),
						erased,
						erasure_claims: yield* database.client.select().from(ThreadErasureClaims),
						threads: yield* database.client.select().from(Threads),
					};
				}),
			);

			expect(result.erased).toEqual([]);
			expect(result.erasure_claims).toEqual([]);
			expect(result.approvals[0]?.state).toBe("requested");
			expect(result.threads.map((thread) => thread.thread_id)).toEqual([thread_id]);
		} finally {
			await runtime.dispose();
		}
	});

	it("erases terminal clone ownership while retaining the registered project", async () => {
		const clock = { value: created_at };
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath), clock);
		const thread_id = "thread_applied_clone";

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const clones = yield* HostedProjectCloneRepository;
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const projects = yield* ProjectRepository;
					const approval_id = "approval_applied_clone";

					yield* SeedThread(thread_id);
					yield* clones.Request(clone_request(thread_id, approval_id));
					yield* clones.Decide({
						approval_id,
						approved: true,
						decision_command: {
							message_id: "decision_applied_clone",
							sent_at: created_at,
						},
						thread_id,
					});
					yield* clones.MarkExecuting(approval_id);
					const execution = yield* clones.ReadExecution(approval_id);
					const identity = { approval_id, claim_token: execution.claim_token };

					yield* clones.ExecuteClaimed(identity, Effect.void);
					yield* clones.RecordCloneResult({ ...identity, result: clone_result() });
					const registered = yield* projects.RegisterHosted(project_registration());

					yield* clones.RecordRegisteredProject({
						...identity,
						project: registered.project,
					});
					const settled = yield* clones.Settle({
						...identity,
						attachment: "attached",
						project: registered.project.project,
						type: "applied",
					});
					yield* database.client.insert(HostedProjectCloneClaims).values({
						approval_id,
						canonical_host: "github.com",
						canonical_root: "C:/projects/editor",
						claimed_at: created_at,
						claim_token: "terminal_clone_claim",
						lease_expires_at: expired_at,
						native_id: "repository_1",
						owner_instance_id: "retired_runtime",
						provider_id: "github",
						thread_id,
					});
					yield* database.client.insert(ThreadProjectAffinityEvidence).values({
						basis_affinity_version: 0,
						evidence_id: "clone_attachment_evidence",
						kind: "thread_metadata",
						observed_at: created_at,
						project_id: registered.project.project.project_id,
						project_json: JSON.stringify(registered.project.project),
						source_event_id: settled.event.message_id,
						source_journal_sequence: settled.event.journal_sequence,
						thread_id,
					});

					const erased = yield* erasure.CleanupExpired(cutoff, deleted_at);

					return {
						approvals: yield* database.client
							.select()
							.from(HostedProjectCloneApprovals),
						artifacts: yield* database.client
							.select()
							.from(HostedProjectCloneArtifacts),
						claims: yield* database.client.select().from(HostedProjectCloneClaims),
						erased,
						evidence: yield* database.client
							.select()
							.from(ThreadProjectAffinityEvidence),
						origins: yield* database.client.select().from(ProjectHostedOrigins),
						project_rows: yield* database.client.select().from(Projects),
						projects: yield* projects.List,
					};
				}),
			);

			expect(result.erased).toEqual([thread_id]);
			expect(result.approvals).toEqual([]);
			expect(result.artifacts).toEqual([]);
			expect(result.claims).toEqual([]);
			expect(result.evidence).toEqual([]);
			expect(result.project_rows).toHaveLength(1);
			expect(result.origins).toHaveLength(1);
			expect(result.projects).toHaveLength(1);
			expect(result.projects[0]!.project.root_path).toBe("C:/projects/editor");
		} finally {
			await runtime.dispose();
		}
	});
});

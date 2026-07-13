import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Cause, Effect, Exit, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	JournalCommands,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
	WorkspaceChangeOperations,
	WorkspaceMutationAuthorities,
	WorkspaceReplaceApprovals,
} from "../../modules/backend/src/persistence/schema";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	WorkspaceChangeDiffService,
	WorkspaceChangeDiffServiceLive,
} from "../../modules/backend/src/workspace/workspace-change-diff-service";
import { WorkspaceChangeRepositoryLive } from "../../modules/backend/src/workspace/workspace-change-repository";
import {
	WorkspaceReplaceApprovalConflict,
	WorkspaceReplaceApprovalInvariant,
	WorkspaceReplaceApprovalRepository,
	WorkspaceReplaceApprovalRepositoryLive,
	WorkspaceReplaceApprovalUnavailable,
} from "../../modules/backend/src/workspace/workspace-replace-approval-repository";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-13T12:00:00.000Z";
const before = "SOURCE_PRIVATE_ALPHA\nSOURCE_PRIVATE_BETA\n";
const after = "REPLACEMENT_PRIVATE_ALPHA\nREPLACEMENT_PRIVATE_GAMMA\n";
const text_encoder = new TextEncoder();

function identity(content: Uint8Array) {
	return {
		algorithm: "sha256" as const,
		byte_count: content.byteLength,
		content_hash: createHash("sha256").update(content).digest("hex"),
	};
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected the Effect to fail");
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-approval-"));
	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "workspace_approval_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_test_${++next_id}`),
		Now: Effect.succeed(now),
	});
}

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(),
		JournalNotifierLive,
	);
	const services = Layer.mergeAll(
		WorkspaceChangeDiffServiceLive,
		WorkspaceChangeRepositoryLive,
		WorkspaceReplaceApprovalRepositoryLive,
	).pipe(Layer.provideMerge(NodeCrypto.layer), Layer.provideMerge(infrastructure));

	return ManagedRuntime.make(services);
}

const operation = {
	action: "replace" as const,
	agent_id: "agent_1",
	change_id: "change_1",
	diff_format_version: 1 as const,
	evidence_recorded: false,
	expected_identity: identity(text_encoder.encode(before)),
	result_identity: identity(text_encoder.encode(after)),
	lifecycle: "claimed" as const,
	message_id: "message_replace",
	path: "src/example.ts",
	raw_origin: { provider: "codex", reference: "origin_1" },
	request_fingerprint: "a".repeat(64),
	run_id: "run_1",
	sent_at: now,
	thread_id: "thread_1",
	workspace_id: "workspace_1",
};

function prepare() {
	return Effect.gen(function* () {
		const service = yield* WorkspaceChangeDiffService;

		return yield* service.Prepare({
			after: text_encoder.encode(after),
			after_identity: operation.result_identity,
			before: text_encoder.encode(before),
			before_identity: operation.expected_identity,
			change_id: operation.change_id,
			message_id: operation.message_id,
			path: operation.path,
			thread_id: operation.thread_id,
			workspace_id: operation.workspace_id,
		});
	});
}

const SeedClaim = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(Threads).values({
		created_at: now,
		thread_id: operation.thread_id,
		title: "Approval test thread",
		title_source: "initial",
		updated_at: now,
	});
	yield* database.client.insert(WorkspaceChangeOperations).values({
		action: operation.action,
		agent_id: operation.agent_id,
		change_id: operation.change_id,
		created_at: now,
		expected_identity_json: JSON.stringify(operation.expected_identity),
		lifecycle: "claimed",
		message_id: operation.message_id,
		raw_origin_json: JSON.stringify(operation.raw_origin),
		request_fingerprint: operation.request_fingerprint,
		result_identity_json: JSON.stringify(operation.result_identity),
		run_id: operation.run_id,
		sent_at: now,
		thread_id: operation.thread_id,
		updated_at: now,
		workspace_id: operation.workspace_id,
		path: operation.path,
	});
	yield* database.client.insert(WorkspaceMutationAuthorities).values({
		agent_id: operation.agent_id,
		approval: null,
		authority_kind: "base_run",
		change_id: operation.change_id,
		created_at: now,
		message_id: operation.message_id,
		run_id: operation.run_id,
		thread_id: operation.thread_id,
		working_directory: "C:/workspace",
		workspace_id: operation.workspace_id,
	});
});

function request(reason = "Replace the private source with the private replacement") {
	return Effect.gen(function* () {
		const repository = yield* WorkspaceReplaceApprovalRepository;
		const prepared_diff = yield* prepare();

		return yield* repository.Request({
			operation,
			policy: "on_request",
			prepared_diff,
			reason,
		});
	});
}

afterEach(async () => {
	await Promise.all(
		temporary_directories.splice(0).map(async (directory) => {
			await import("node:fs/promises").then(({ rm }) =>
				rm(directory, { force: true, recursive: true }),
			);
		}),
	);
});

describe("WorkspaceReplaceApprovalRepositoryLive", () => {
	it("persists exact private diffs while keeping approval events source-free", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedClaim;
					const prepared = yield* prepare();
					const accepted = yield* request();
					const database = yield* Database;
					const rows = yield* database.client.select().from(WorkspaceReplaceApprovals);
					const events = yield* database.client.select().from(JournalEvents);
					const query = yield* (yield* WorkspaceReplaceApprovalRepository).Query({
						approval_id: accepted.approval.approval_id,
						thread_id: operation.thread_id,
					});

					return { accepted, events, prepared, query, rows };
				}),
			);

			expect(result.accepted.status).toBe("accepted");
			expect(result.rows).toHaveLength(1);
			expect(result.rows[0]?.patch).toEqual(Buffer.from(result.prepared.patch));
			expect(result.query.diff).toEqual({
				added_line_count: result.prepared.added_line_count,
				after_identity: result.prepared.after_identity,
				before_identity: result.prepared.before_identity,
				change_id: result.prepared.change_id,
				context_lines: result.prepared.context_lines,
				format: result.prepared.format,
				format_version: result.prepared.format_version,
				patch: new TextDecoder().decode(result.prepared.patch),
				patch_identity: result.prepared.patch_identity,
				path: result.prepared.path,
				removed_line_count: result.prepared.removed_line_count,
				thread_id: result.prepared.thread_id,
				truncated: false,
				workspace_id: result.prepared.workspace_id,
			});
			expect(JSON.stringify(result.events[0])).not.toContain(before);
			expect(JSON.stringify(result.events[0])).not.toContain(after);
		} finally {
			await runtime.dispose();
		}
	});

	it("converges exact request retries and rejects request conflicts", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedClaim;
					const first = yield* request();
					const duplicate = yield* request();
					const conflict = yield* request("A different reason").pipe(Effect.exit);

					return { conflict, duplicate, first };
				}),
			);

			expect(result.first.status).toBe("accepted");
			expect(result.duplicate).toEqual({ ...result.first, status: "duplicate" });
			expect(failure_from(result.conflict)).toEqual(
				new WorkspaceReplaceApprovalConflict({ reason: "request_conflict" }),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("accepts null base-run policy as on_request and owns decision command/event state", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedClaim;
					const repository = yield* WorkspaceReplaceApprovalRepository;
					const accepted = yield* request();
					const decided = yield* repository.Decide({
						approval_id: accepted.approval.approval_id,
						approved: true,
						message_id: "decision_1",
						sent_at: now,
						thread_id: operation.thread_id,
					});
					const duplicate = yield* repository.Decide({
						approval_id: accepted.approval.approval_id,
						approved: true,
						message_id: "decision_1",
						sent_at: now,
						thread_id: operation.thread_id,
					});
					const opposing = yield* repository
						.Decide({
							approval_id: accepted.approval.approval_id,
							approved: false,
							message_id: "decision_2",
							sent_at: now,
							thread_id: operation.thread_id,
						})
						.pipe(Effect.exit);
					const database = yield* Database;
					const commands = yield* database.client.select().from(JournalCommands);
					const events = yield* database.client.select().from(JournalEvents);

					return { commands, decided, duplicate, events, opposing };
				}),
			);

			expect(result.decided.approval.policy).toBe("on_request");
			expect(result.decided.approval.state).toBe("approved");
			expect(result.duplicate.status).toBe("duplicate");
			expect(failure_from(result.opposing)).toEqual(
				new WorkspaceReplaceApprovalConflict({ reason: "decision_conflict" }),
			);
			expect(result.commands).toHaveLength(1);
			expect(result.commands[0]?.payload_json).toBe(
				JSON.stringify({
					approval_id: result.decided.approval.approval_id,
					approved: true,
					type: "workspace.replace_approval.response",
				}),
			);
			expect(JSON.stringify(result.commands[0])).not.toContain(before);
			expect(JSON.stringify(result.commands[0])).not.toContain(after);
			expect(JSON.stringify(result.events)).not.toContain(before);
			expect(JSON.stringify(result.events)).not.toContain(after);
		} finally {
			await runtime.dispose();
		}
	});

	it("supports legal execution transitions, executable reads, and rejects invalid transitions", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedClaim;
					const repository = yield* WorkspaceReplaceApprovalRepository;
					const requested = yield* request();
					yield* repository.Decide({
						approval_id: requested.approval.approval_id,
						approved: true,
						message_id: "decision_1",
						sent_at: now,
						thread_id: operation.thread_id,
					});
					const executing = yield* repository.MarkExecuting(
						requested.approval.approval_id,
					);
					const executing_duplicate = yield* repository.MarkExecuting(
						requested.approval.approval_id,
					);
					const execution = yield* repository.ReadExecution(
						requested.approval.approval_id,
					);
					const executable = yield* repository.ListExecutable;
					const invalid = yield* repository
						.MarkRejected(requested.approval.approval_id)
						.pipe(Effect.exit);
					const database = yield* Database;
					yield* database.client.run(
						`UPDATE workspace_change_operations SET lifecycle = 'committed' WHERE message_id = '${operation.message_id}'`,
					);
					const applied = yield* repository.MarkApplied(requested.approval.approval_id);

					return {
						applied,
						executable,
						executing,
						executing_duplicate,
						execution,
						invalid,
					};
				}),
			);

			expect(result.executing.approval.state).toBe("executing");
			expect(result.executing_duplicate.status).toBe("duplicate");
			expect(result.execution.prepared_diff.patch).toEqual(
				(await runtime.runPromise(prepare())).patch,
			);
			expect(result.executable).toEqual([result.executing.approval.approval_id]);
			expect(failure_from(result.invalid)).toEqual(
				new WorkspaceReplaceApprovalInvariant({
					message: expect.stringContaining("does not match its operation lifecycle"),
				}),
			);
			expect(result.applied.approval.state).toBe("applied");
		} finally {
			await runtime.dispose();
		}
	});

	it("supports denial and rejects new decisions after a terminal denial", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedClaim;
					const repository = yield* WorkspaceReplaceApprovalRepository;
					const requested = yield* request();
					const denied = yield* repository.Decide({
						approval_id: requested.approval.approval_id,
						approved: false,
						message_id: "decision_denied",
						sent_at: now,
						thread_id: operation.thread_id,
					});
					const duplicate = yield* repository.Decide({
						approval_id: requested.approval.approval_id,
						approved: false,
						message_id: "decision_denied",
						sent_at: now,
						thread_id: operation.thread_id,
					});
					const new_id = yield* repository
						.Decide({
							approval_id: requested.approval.approval_id,
							approved: false,
							message_id: "decision_other",
							sent_at: now,
							thread_id: operation.thread_id,
						})
						.pipe(Effect.exit);

					return { denied, duplicate, new_id };
				}),
			);

			expect(result.denied.approval.state).toBe("denied");
			expect(result.duplicate.status).toBe("duplicate");
			expect(failure_from(result.new_id)).toEqual(
				new WorkspaceReplaceApprovalConflict({ reason: "decision_conflict" }),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed for corrupt private state, unavailable threads, and invalid migration shapes", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedClaim;
					const repository = yield* WorkspaceReplaceApprovalRepository;
					const requested = yield* request();
					const database = yield* Database;
					yield* database.client.run(
						`UPDATE workspace_replace_approvals SET patch_hash = '${"f".repeat(64)}' WHERE approval_id = '${requested.approval.approval_id}'`,
					);
					const corrupt = yield* repository
						.Query({
							approval_id: requested.approval.approval_id,
							thread_id: operation.thread_id,
						})
						.pipe(Effect.exit);
					const invalid_hash = yield* database.client
						.run(
							`UPDATE workspace_replace_approvals SET patch_hash = '${"g".repeat(64)}' WHERE approval_id = '${requested.approval.approval_id}'`,
						)
						.pipe(Effect.exit);
					yield* database.client
						.insert(ThreadErasureClaims)
						.values({ claimed_at: now, thread_id: operation.thread_id });
					const unavailable = yield* repository
						.Query({
							approval_id: requested.approval.approval_id,
							thread_id: operation.thread_id,
						})
						.pipe(Effect.exit);

					return { corrupt, invalid_hash, unavailable };
				}),
			);

			expect(failure_from(result.corrupt)).toBeInstanceOf(WorkspaceReplaceApprovalInvariant);
			expect(Exit.isFailure(result.invalid_hash)).toBe(true);
			expect(failure_from(result.unavailable)).toEqual(
				new WorkspaceReplaceApprovalUnavailable({ reason: "erased" }),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("enforces additive approval policy, state, and decision constraints in SQLite", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedClaim;
					const accepted = yield* request();
					const database = yield* Database;
					yield* database.client.insert(WorkspaceChangeOperations).values(
						["policy", "state", "decision"].map((suffix) => ({
							action: "replace" as const,
							agent_id: operation.agent_id,
							change_id: `invalid_${suffix}`,
							created_at: now,
							diff_format_version: 1 as const,
							evidence_recorded: false,
							expected_identity_json: JSON.stringify(operation.expected_identity),
							lifecycle: "claimed" as const,
							message_id: `message_invalid_${suffix}`,
							raw_origin_json: null,
							request_fingerprint: "b".repeat(64),
							result_identity_json: JSON.stringify(operation.result_identity),
							run_id: operation.run_id,
							sent_at: now,
							thread_id: operation.thread_id,
							updated_at: now,
							workspace_id: operation.workspace_id,
							path: operation.path,
						})),
					);
					const base = {
						...(yield* database.client.select().from(WorkspaceReplaceApprovals))[0]!,
						approval_id: "invalid",
						change_id: "invalid_change",
					};
					const invalid_policy = yield* database.client
						.insert(WorkspaceReplaceApprovals)
						.values({
							...base,
							change_id: "invalid_policy",
							message_id: "message_invalid_policy",
							policy: "never",
						})
						.pipe(Effect.exit);
					const invalid_state = yield* database.client
						.insert(WorkspaceReplaceApprovals)
						.values({
							...base,
							approval_id: "invalid_state",
							change_id: "invalid_state",
							message_id: "message_invalid_state",
							policy: "on_request",
							state: "unknown",
						})
						.pipe(Effect.exit);
					const invalid_decision = yield* database.client
						.insert(WorkspaceReplaceApprovals)
						.values({
							...base,
							approval_id: "invalid_decision",
							change_id: "invalid_decision",
							message_id: "message_invalid_decision",
							policy: "on_request",
							state: "requested",
							approved: true,
						})
						.pipe(Effect.exit);

					return { accepted, invalid_decision, invalid_policy, invalid_state };
				}),
			);

			expect(result.accepted.approval.state).toBe("requested");
			expect(Exit.isFailure(result.invalid_policy)).toBe(true);
			expect(Exit.isFailure(result.invalid_state)).toBe(true);
			expect(Exit.isFailure(result.invalid_decision)).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("converges concurrent exact requests and decisions", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedClaim;
					const repository = yield* WorkspaceReplaceApprovalRepository;
					const requests = yield* Effect.all([request(), request()], {
						concurrency: "unbounded",
					});
					const decisions = yield* Effect.all(
						requests.map(() =>
							repository.Decide({
								approval_id: requests[0]!.approval.approval_id,
								approved: true,
								message_id: "decision_concurrent",
								sent_at: now,
								thread_id: operation.thread_id,
							}),
						),
						{ concurrency: "unbounded" },
					);

					return { decisions, requests };
				}),
			);

			expect(result.requests.map(({ status }) => status).sort()).toEqual([
				"accepted",
				"duplicate",
			]);
			expect(result.decisions.map(({ status }) => status).sort()).toEqual([
				"accepted",
				"duplicate",
			]);
		} finally {
			await runtime.dispose();
		}
	});
});

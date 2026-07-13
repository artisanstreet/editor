import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Exit, Fiber, Layer, Redacted, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	AckEnvelope,
	CommandEnvelope,
	ContentIdentity,
	HelloEnvelope,
	OutboundControlEnvelope,
	SubscribeEnvelope,
	ThreadListQueryEnvelope,
	WorkspaceChangeDiffQueryEnvelope,
	WorkspaceChangeListQueryEnvelope,
	WorkspaceChangeReviewEnvelope,
	WorkspaceChangeRollbackEnvelope,
	WorkspaceFileReadQueryEnvelope,
	WorkspaceFileReplaceEnvelope,
	WorkspaceReplaceApprovalQueryEnvelope,
	WorkspaceReplaceApprovalRespondEnvelope,
} from "@artisan/protocol";
import {
	DecodeProtocolConnectionOptions,
	make_backend_runtime,
	make_workspace_bounded_regular_file_store_registry_layer,
	ProtocolServer,
	WorkspaceChangeRepository,
	WorkspaceFileService,
	WorkspaceReplaceApprovalCoordinator,
	WorkspaceReplaceApprovalRepository,
	type ProtocolConnection,
} from "@artisan/backend";

import { ProtocolRouter } from "../../modules/backend/src/protocol/protocol-router";
import { Database } from "../../modules/backend/src/persistence/database";
import {
	JournalEvents,
	OrchestrationCoordinators,
	OrchestrationRuns,
	WorkspaceChangeOperations,
	WorkspaceChangeSnapshots,
	WorkspaceChanges,
	WorkspaceMutationPayloads,
	WorkspaceReplaceApprovals,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const temporary_directories: Array<string> = [];
const workspace_time = "2026-07-12T14:00:00.000Z";
const workspace_receipt_key = Redacted.make(new Uint8Array(32).fill(7));
const workspace_encoder = new TextEncoder();

interface NativeReplacementOptions {
	readonly expected: Uint8Array;
	readonly maximumBytes: number;
	readonly operationId: string;
	readonly path: string;
	readonly replacement: Uint8Array;
}

function content_identity(content: string): ContentIdentity {
	const bytes = workspace_encoder.encode(content);

	return {
		algorithm: "sha256",
		byte_count: bytes.byteLength,
		content_hash: createHash("sha256").update(bytes).digest("hex"),
	};
}

function bytes_match(left: Uint8Array, right: Uint8Array) {
	return (
		left.byteLength === right.byteLength && left.every((value, index) => value === right[index])
	);
}

function replacement_options_match(
	left: NativeReplacementOptions,
	right: NativeReplacementOptions,
) {
	return (
		left.maximumBytes === right.maximumBytes &&
		left.operationId === right.operationId &&
		left.path === right.path &&
		bytes_match(left.expected, right.expected) &&
		bytes_match(left.replacement, right.replacement)
	);
}

function make_workspace_native_module(root: string) {
	const receipts = new Map<string, NativeReplacementOptions>();

	class FakeNativeBoundedRegularFileStore {
		constructor(
			readonly configured_root: string,
			_receipt_authentication_key: Uint8Array,
		) {}

		authorizeRoot(candidate_root: string) {
			return Promise.resolve(candidate_root === this.configured_root);
		}

		close() {}

		async finalizeRegularFileReplacement(options: NativeReplacementOptions) {
			const receipt = receipts.get(options.operationId);

			if (receipt === undefined || !replacement_options_match(receipt, options)) {
				throw new Error("replacement receipt is unavailable");
			}

			receipts.delete(options.operationId);
		}

		async readRegularFile(path: string, maximum_bytes: number) {
			const bytes = new Uint8Array(await readFile(join(root, path)));

			if (bytes.byteLength > maximum_bytes) {
				throw new Error("file exceeds maximum bytes");
			}

			return bytes;
		}

		async replaceRegularFile(options: NativeReplacementOptions) {
			const receipt = receipts.get(options.operationId);

			if (receipt !== undefined) {
				if (!replacement_options_match(receipt, options)) {
					throw new Error("replacement receipt intent changed");
				}

				return "AlreadyReplaced";
			}

			const target = join(root, options.path);
			const current = new Uint8Array(await readFile(target));
			const matches =
				current.byteLength === options.expected.byteLength &&
				current.every((value, index) => value === options.expected[index]);

			if (!matches || options.replacement.byteLength > options.maximumBytes) {
				return "Changed";
			}

			await writeFile(target, options.replacement);
			receipts.set(options.operationId, {
				...options,
				expected: Uint8Array.from(options.expected),
				replacement: Uint8Array.from(options.replacement),
			});

			return "Replaced";
		}
	}

	return () => ({
		NativeBoundedRegularFileStore: FakeNativeBoundedRegularFileStore,
		getNativeBuildDescriptor: () => ({
			architecture: "x86_64",
			operatingSystem: "windows",
			target: "x86_64-pc-windows-msvc",
			testHooksEnabled: false,
		}),
	});
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-editor-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

async function make_workspace_runtime() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-editor-workspace-protocol-"));
	const root = join(directory, "workspace");
	const database_path = join(directory, "artisan.db");

	temporary_directories.push(directory);
	await mkdir(join(root, "src"), { recursive: true });
	await writeFile(join(root, "src", "example.ts"), "before");

	return {
		database_path,
		root,
		runtime: make_workspace_backend_runtime(database_path, root),
	};
}

function make_workspace_backend_runtime(database_path: string, root: string) {
	return make_backend_runtime({
		database_path,
		migrations_path,
		workspace_bounded_regular_file_store_registry:
			make_workspace_bounded_regular_file_store_registry_layer(
				[{ root, workspace_id: "workspace_protocol" }],
				{
					load_native_module: make_workspace_native_module(root),
					receipt_authentication_key: workspace_receipt_key,
				},
			).pipe(Layer.provide(NodeFileSystem.layer)),
	});
}

function requested_approval_id(envelopes: ReadonlyArray<OutboundControlEnvelope>) {
	const event = envelopes.find(
		(envelope) =>
			envelope.kind === "event" &&
			envelope.payload.type === "workspace.replace.approval.updated",
	);

	if (event?.kind !== "event" || event.payload.type !== "workspace.replace.approval.updated") {
		throw new Error("Approval request event was not delivered");
	}

	return event.payload.approval.approval_id;
}

function command_receipt_sequence(envelopes: ReadonlyArray<OutboundControlEnvelope>) {
	const receipt = envelopes.find((envelope) => envelope.kind === "command.receipt");

	if (
		receipt?.kind !== "command.receipt" ||
		receipt.payload.status === "rejected" ||
		receipt.payload.journal_sequence === undefined
	) {
		throw new Error("Durable command receipt was not delivered");
	}

	return receipt.payload.journal_sequence;
}

function WaitForApprovalState(
	approval_id: string,
	state: "applied" | "approved" | "denied" | "rejected",
) {
	return Effect.gen(function* () {
		const approvals = yield* WorkspaceReplaceApprovalRepository;

		for (let attempt = 0; attempt < 200; attempt += 1) {
			const result = yield* approvals.Query({
				approval_id,
				thread_id: "thread_workspace_protocol",
			});

			if (result.approval.state === state) {
				return result;
			}

			yield* Effect.sleep("10 millis");
		}

		return yield* Effect.die(`Approval ${approval_id} did not reach ${state}`);
	});
}

function WaitForDeniedSettlement(approval_id: string) {
	return Effect.gen(function* () {
		const approvals = yield* WorkspaceReplaceApprovalRepository;

		for (let attempt = 0; attempt < 200; attempt += 1) {
			const unsettled = yield* approvals.ListDeniedUnsettled;

			if (!unsettled.includes(approval_id)) {
				return;
			}

			yield* Effect.sleep("10 millis");
		}

		return yield* Effect.die(`Denied approval ${approval_id} did not settle`);
	});
}

function workspace_replace(
	message_id: string,
	change_id: string,
	content: string,
	expected_before: ContentIdentity = content_identity("before"),
	approval_reason?: string,
): WorkspaceFileReplaceEnvelope {
	return {
		agent_id: "agent_workspace_protocol",
		kind: "workspace.file.replace",
		message_id,
		origin: "frontend",
		payload: {
			...(approval_reason === undefined
				? {}
				: { approval_request: { reason: approval_reason } }),
			change_id,
			content,
			expected_before,
			path: "src/example.ts",
			workspace_id: "workspace_protocol",
		},
		protocol_version: 1,
		raw_origin: { provider: "codex", reference: "native_workspace_protocol" },
		run_id: "run_workspace_protocol",
		schema_version: 1,
		sent_at: workspace_time,
		thread_id: "thread_workspace_protocol",
	};
}

function workspace_approval_query(
	message_id: string,
	approval_id: string,
): WorkspaceReplaceApprovalQueryEnvelope {
	return {
		kind: "workspace.replace.approval.query",
		message_id,
		origin: "frontend",
		payload: { approval_id, thread_id: "thread_workspace_protocol" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
	};
}

function workspace_approval_response(
	message_id: string,
	approval_id: string,
	approved: boolean,
): WorkspaceReplaceApprovalRespondEnvelope {
	return {
		kind: "workspace.replace.approval.respond",
		message_id,
		origin: "frontend",
		payload: { approval_id, approved },
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
		thread_id: "thread_workspace_protocol",
	};
}

function workspace_read(message_id: string): WorkspaceFileReadQueryEnvelope {
	return {
		kind: "workspace.file.read.query",
		message_id,
		origin: "frontend",
		payload: { path: "src/example.ts", workspace_id: "workspace_protocol" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
	};
}

function workspace_list(message_id: string): WorkspaceChangeListQueryEnvelope {
	return {
		kind: "workspace.change.list.query",
		message_id,
		origin: "frontend",
		payload: { thread_id: "thread_workspace_protocol", workspace_id: "workspace_protocol" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
	};
}

function workspace_diff(
	message_id: string,
	change_id = "change_workspace_protocol",
): WorkspaceChangeDiffQueryEnvelope {
	return {
		kind: "workspace.change.diff.query",
		message_id,
		origin: "frontend",
		payload: { change_id, thread_id: "thread_workspace_protocol" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
	};
}

function workspace_review(
	message_id: string,
	change_id = "change_workspace_protocol",
): WorkspaceChangeReviewEnvelope {
	return {
		kind: "workspace.change.review",
		message_id,
		origin: "frontend",
		payload: { change_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
		thread_id: "thread_workspace_protocol",
	};
}

function workspace_rollback(message_id: string): WorkspaceChangeRollbackEnvelope {
	return {
		kind: "workspace.change.rollback",
		message_id,
		origin: "frontend",
		payload: {
			change_id: "change_workspace_protocol",
			expected_after: content_identity("after"),
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: workspace_time,
		thread_id: "thread_workspace_protocol",
	};
}

function seed_workspace_run(root: string) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const router = yield* ProtocolRouter;

		yield* router.Route({
			kind: "command",
			message_id: "create_workspace_protocol_thread",
			origin: "frontend",
			payload: { title: "Workspace protocol", type: "thread.create" },
			protocol_version: 1,
			schema_version: 1,
			sent_at: workspace_time,
			thread_id: "thread_workspace_protocol",
		});
		yield* database.client.insert(OrchestrationCoordinators).values({
			active_run_id: "run_workspace_protocol",
			agent_id: "agent_workspace_protocol",
			created_at: workspace_time,
			display_name: "Coordinator",
			engine_id: "engine_workspace_protocol",
			role: "primary",
			thread_id: "thread_workspace_protocol",
			updated_at: workspace_time,
		});
		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: "agent_workspace_protocol",
			created_at: workspace_time,
			engine_id: "engine_workspace_protocol",
			run_id: "run_workspace_protocol",
			status: "running",
			thread_id: "thread_workspace_protocol",
			updated_at: workspace_time,
			working_directory: root,
		});
	});
}

function terminalize_workspace_run() {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client
			.update(OrchestrationRuns)
			.set({ status: "complete", updated_at: workspace_time });
	});
}

function make_hello(
	message_id = "hello_1",
	last_journal_sequence = 0,
	event_cursors: HelloEnvelope["payload"]["event_cursors"] = [],
	supported_protocol_versions: HelloEnvelope["payload"]["supported_protocol_versions"] = [1],
): HelloEnvelope {
	return {
		kind: "hello",
		message_id,
		origin: "frontend",
		payload: {
			event_cursors,
			last_journal_sequence,
			supported_protocol_versions,
		},
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function make_command(
	message_id = "command_1",
	thread_id = "thread_1",
	title = "Protocol integration",
): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: {
			title,
			type: "thread.create",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		thread_id,
	};
}

function make_query(message_id = "query_1"): ThreadListQueryEnvelope {
	return {
		kind: "thread.list.query",
		message_id,
		origin: "frontend",
		payload: {},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function make_subscribe(message_id = "subscribe_1"): SubscribeEnvelope {
	return {
		kind: "subscribe",
		message_id,
		origin: "frontend",
		payload: { type: "thread.list" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		subscription_id: "subscription_1",
	};
}

function make_ack(
	message_id: string,
	journal_sequence: number,
	event_cursors: AckEnvelope["payload"]["event_cursors"],
): AckEnvelope {
	return {
		kind: "ack",
		message_id,
		origin: "frontend",
		payload: { event_cursors, journal_sequence },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

const open_connection = Effect.gen(function* () {
	const protocol_server = yield* ProtocolServer;

	return yield* protocol_server.Open;
});

function take_outbound(connection: ProtocolConnection, count: number) {
	return connection.Outbound.pipe(Stream.take(count), Stream.runCollect);
}

function take_until_outbound(
	connection: ProtocolConnection,
	predicate: (envelope: OutboundControlEnvelope) => boolean,
) {
	return connection.Outbound.pipe(Stream.takeUntil(predicate), Stream.runCollect);
}

const negotiate = (connection: ProtocolConnection, hello = make_hello()) =>
	Effect.gen(function* () {
		yield* connection.Receive(hello);

		return yield* take_until_outbound(
			connection,
			(envelope) => envelope.kind === "replay.complete",
		);
	});

afterEach(async () => {
	await Promise.all(
		temporary_directories.splice(0).map((directory) =>
			rm(directory, {
				force: true,
				recursive: true,
			}),
		),
	);
});

describe("protocol server", () => {
	it("arbitrates concurrent same-id replacements with different approval intent", async () => {
		const { root, runtime } = await make_workspace_runtime();

		try {
			await runtime.runPromise(seed_workspace_run(root));
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const workspace_files = yield* WorkspaceFileService;
					const common = {
						agent_id: "agent_workspace_protocol",
						change_id: "change_workspace_mixed_intent",
						content: "mixed intent content",
						expected_before: content_identity("before"),
						message_id: "workspace_replace_mixed_intent",
						path: "src/example.ts",
						run_id: "run_workspace_protocol",
						sent_at: workspace_time,
						thread_id: "thread_workspace_protocol",
						workspace_id: "workspace_protocol",
					};
					const outcomes = yield* Effect.all(
						[
							workspace_files
								.Replace({
									...common,
									approval_request: { reason: "Require explicit approval." },
								})
								.pipe(Effect.exit),
							workspace_files.Replace(common).pipe(Effect.exit),
						],
						{ concurrency: "unbounded" },
					);
					const database = yield* Database;
					const approvals = yield* database.client
						.select()
						.from(WorkspaceReplaceApprovals);
					const content = yield* Effect.promise(() =>
						readFile(join(root, "src", "example.ts"), "utf8"),
					);

					return { approvals, content, outcomes };
				}),
			);

			expect(result.outcomes.filter(Exit.isSuccess)).toHaveLength(1);
			expect(result.outcomes.filter(Exit.isFailure)).toHaveLength(1);
			expect(result.approvals).toHaveLength(result.content === "before" ? 1 : 0);
			expect(result.content).toMatch(/^(before|mixed intent content)$/);
		} finally {
			await runtime.dispose();
		}
	});

	it("routes controlled workspace queries and mutations through correlated receipts", async () => {
		const { root, runtime } = await make_workspace_runtime();

		try {
			await runtime.runPromise(seed_workspace_run(root));
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive(workspace_read("workspace_read"));
						const read = yield* take_outbound(connection, 1);

						yield* connection.Receive(workspace_list("workspace_list_empty"));
						const empty_list = yield* take_outbound(connection, 1);
						yield* connection.Receive(
							workspace_diff("workspace_diff_missing", "change_workspace_missing"),
						);
						const diff_missing = yield* take_outbound(connection, 1);

						yield* Effect.promise(() =>
							writeFile(join(root, "src", "example.ts"), "external"),
						);
						yield* connection.Receive(
							workspace_replace(
								"workspace_replace_conflict",
								"change_workspace_conflict",
								"after",
							),
						);
						const conflict = yield* take_outbound(connection, 1);
						yield* connection.Receive(
							workspace_review(
								"workspace_review_unavailable",
								"change_workspace_missing",
							),
						);
						const review_unavailable = yield* take_outbound(connection, 1);

						yield* Effect.promise(() =>
							writeFile(join(root, "src", "example.ts"), "before"),
						);
						yield* connection.Receive(
							workspace_replace(
								"workspace_replace",
								"change_workspace_protocol",
								"after",
							),
						);
						const replace = yield* take_outbound(connection, 2);

						yield* connection.Receive(
							workspace_replace(
								"workspace_replace",
								"change_workspace_protocol",
								"after",
							),
						);
						const replace_duplicate = yield* take_outbound(connection, 1);

						yield* connection.Receive(workspace_list("workspace_list_recorded"));
						const recorded_list = yield* take_outbound(connection, 1);
						yield* connection.Receive(workspace_diff("workspace_diff_recorded"));
						const recorded_diff = yield* take_outbound(connection, 1);

						yield* terminalize_workspace_run();
						yield* connection.Receive(workspace_review("workspace_review"));
						const review = yield* take_outbound(connection, 2);

						yield* connection.Receive(workspace_review("workspace_review"));
						const review_duplicate = yield* take_outbound(connection, 1);

						yield* connection.Receive(workspace_rollback("workspace_rollback"));
						const rollback = yield* take_outbound(connection, 2);

						yield* connection.Receive(workspace_rollback("workspace_rollback"));
						const rollback_duplicate = yield* take_outbound(connection, 1);

						yield* connection.Receive({
							...workspace_read("workspace_read_unavailable"),
							payload: { path: "src/example.ts", workspace_id: "missing_workspace" },
						});
						const read_unavailable = yield* take_outbound(connection, 1);
						const database = yield* Database;

						yield* database.client.insert(WorkspaceChanges).values({
							after_identity_json: "{}",
							agent_id: "agent_workspace_protocol",
							before_identity_json: "{}",
							change_id: "change_workspace_invalid",
							created_at: workspace_time,
							path: "src/example.ts",
							raw_origin_json: null,
							review_state: "rolled_back",
							reviewed_at: workspace_time,
							rollback_state: "consumed",
							rolled_back_at: workspace_time,
							run_id: "run_workspace_protocol",
							source_command_id: "workspace_invalid_command",
							thread_id: "thread_workspace_protocol",
							updated_at: workspace_time,
							version: 1,
							workspace_id: "workspace_protocol",
						});
						yield* connection.Receive(workspace_list("workspace_list_unavailable"));
						const list_unavailable = yield* take_outbound(connection, 1);

						return {
							conflict,
							diff_missing,
							empty_list,
							list_unavailable,
							read,
							read_unavailable,
							recorded_diff,
							recorded_list,
							replace,
							replace_duplicate,
							review,
							review_duplicate,
							review_unavailable,
							rollback,
							rollback_duplicate,
						};
					}),
				),
			);

			expect(output.read).toMatchObject([
				{
					correlation_id: "workspace_read",
					kind: "workspace.file.read.query.result",
					payload: { content: "before", identity: content_identity("before") },
				},
			]);
			expect(output.empty_list).toMatchObject([
				{
					correlation_id: "workspace_list_empty",
					kind: "workspace.change.list.query.result",
					payload: { changes: [] },
				},
			]);
			expect(output.diff_missing).toMatchObject([
				{
					correlation_id: "workspace_diff_missing",
					kind: "protocol.error",
					payload: { code: "workspace.diff_unavailable", retryable: false },
				},
			]);
			expect(output.conflict).toMatchObject([
				{
					causation_id: "workspace_replace_conflict",
					correlation_id: "workspace_replace_conflict",
					kind: "command.receipt",
					payload: {
						error: { code: "workspace.conflict", retryable: false },
						status: "rejected",
					},
					thread_id: "thread_workspace_protocol",
				},
			]);
			expect(output.replace).toMatchObject([
				{
					causation_id: "workspace_replace",
					correlation_id: "workspace_replace",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "accepted" },
				},
				{
					agent_id: "agent_workspace_protocol",
					correlation_id: "workspace_replace",
					kind: "event",
					payload: {
						action: "recorded",
						change: {
							raw_origin: {
								provider: "codex",
								reference: "native_workspace_protocol",
							},
						},
						type: "workspace.change.updated",
					},
					raw_origin: { provider: "codex", reference: "native_workspace_protocol" },
					run_id: "run_workspace_protocol",
					thread_id: "thread_workspace_protocol",
				},
			]);
			expect(output.replace_duplicate).toMatchObject([
				{
					correlation_id: "workspace_replace",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "duplicate" },
				},
			]);
			expect(output.recorded_list).toMatchObject([
				{
					correlation_id: "workspace_list_recorded",
					kind: "workspace.change.list.query.result",
					payload: {
						changes: [
							{
								change_id: "change_workspace_protocol",
								review_state: "needs_review",
							},
						],
					},
				},
			]);
			expect(output.recorded_diff).toMatchObject([
				{
					correlation_id: "workspace_diff_recorded",
					kind: "workspace.change.diff.query.result",
					payload: {
						added_line_count: 1,
						after_identity: content_identity("after"),
						before_identity: content_identity("before"),
						change_id: "change_workspace_protocol",
						context_lines: 3,
						format: "unified",
						format_version: 1,
						patch: expect.stringContaining(
							"--- a/src/example.ts\n+++ b/src/example.ts\n",
						),
						patch_identity: {
							algorithm: "sha256",
							byte_count: expect.any(Number),
							content_hash: expect.stringMatching(/^[a-f0-9]{64}$/),
						},
						removed_line_count: 1,
						truncated: false,
						workspace_id: "workspace_protocol",
					},
				},
			]);
			expect(output.review).toMatchObject([
				{
					correlation_id: "workspace_review",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "accepted" },
				},
				{
					correlation_id: "workspace_review",
					kind: "event",
					payload: { action: "reviewed" },
				},
			]);
			expect(output.review_duplicate).toMatchObject([
				{
					correlation_id: "workspace_review",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "duplicate" },
				},
			]);
			expect(output.review_unavailable).toMatchObject([
				{
					causation_id: "workspace_review_unavailable",
					correlation_id: "workspace_review_unavailable",
					kind: "command.receipt",
					payload: {
						error: { code: "workspace.unavailable", retryable: true },
						status: "rejected",
					},
					thread_id: "thread_workspace_protocol",
				},
			]);
			expect(output.rollback).toMatchObject([
				{
					correlation_id: "workspace_rollback",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "accepted" },
				},
				{
					correlation_id: "workspace_rollback",
					kind: "event",
					payload: { action: "rolled_back" },
				},
			]);
			expect(output.rollback_duplicate).toMatchObject([
				{
					correlation_id: "workspace_rollback",
					kind: "command.receipt",
					payload: { journal_sequence: expect.any(Number), status: "duplicate" },
				},
			]);
			expect(output.read_unavailable).toMatchObject([
				{
					correlation_id: "workspace_read_unavailable",
					kind: "protocol.error",
					payload: { code: "workspace.unavailable", retryable: true },
				},
			]);
			expect(output.list_unavailable).toMatchObject([
				{
					correlation_id: "workspace_list_unavailable",
					kind: "protocol.error",
					payload: { code: "projection.unavailable", retryable: true },
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("durably queries, approves, and executes a controlled workspace replacement", async () => {
		const { root, runtime } = await make_workspace_runtime();

		try {
			await runtime.runPromise(seed_workspace_run(root));
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive(
							workspace_replace(
								"workspace_replace_approval",
								"change_workspace_approval",
								"approved content",
								content_identity("before"),
								"Apply the prepared file replacement.",
							),
						);
						const requested = yield* take_outbound(connection, 2);
						const approval_id = requested_approval_id(requested);
						const request_sequence = command_receipt_sequence(requested);

						yield* connection.Receive(
							workspace_approval_query("workspace_approval_query", approval_id),
						);
						const query = yield* take_outbound(connection, 1);
						yield* connection.Receive(
							workspace_replace(
								"workspace_replace_approval",
								"change_workspace_approval",
								"approved content",
							),
						);
						const omitted_request = yield* take_outbound(connection, 1);
						yield* connection.Receive(
							workspace_replace(
								"workspace_replace_approval",
								"change_workspace_approval",
								"approved content",
								content_identity("before"),
								"A conflicting approval reason.",
							),
						);
						const changed_request = yield* take_outbound(connection, 1);

						yield* connection.Receive(
							workspace_approval_response(
								"workspace_approval_response",
								approval_id,
								true,
							),
						);
						const approved = yield* take_until_outbound(
							connection,
							(envelope) =>
								envelope.kind === "event" &&
								envelope.payload.type === "workspace.replace.approval.updated" &&
								envelope.payload.approval.state === "applied",
						);

						yield* connection.Receive(
							workspace_approval_query("workspace_approval_applied", approval_id),
						);
						const applied = yield* take_outbound(connection, 1);
						yield* connection.Receive(
							workspace_replace(
								"workspace_replace_approval",
								"change_workspace_approval",
								"approved content",
								content_identity("before"),
								"Apply the prepared file replacement.",
							),
						);
						const replace_replay = yield* take_outbound(connection, 1);

						yield* connection.Receive(
							workspace_approval_response(
								"workspace_approval_response",
								approval_id,
								true,
							),
						);
						const duplicate = yield* take_outbound(connection, 1);
						const content = yield* Effect.promise(() =>
							readFile(join(root, "src", "example.ts"), "utf8"),
						);

						return {
							applied,
							approved,
							changed_request,
							content,
							duplicate,
							omitted_request,
							query,
							replace_replay,
							request_sequence,
							requested,
						};
					}),
				),
			);

			expect(output.requested).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						kind: "command.receipt",
						payload: expect.objectContaining({ status: "accepted" }),
					}),
					expect.objectContaining({
						kind: "event",
						payload: expect.objectContaining({
							approval: expect.objectContaining({
								reason: "Apply the prepared file replacement.",
								state: "requested",
							}),
							type: "workspace.replace.approval.updated",
						}),
					}),
				]),
			);
			expect(output.query).toMatchObject([
				{
					correlation_id: "workspace_approval_query",
					kind: "workspace.replace.approval.query.result",
					payload: {
						approval: { state: "requested" },
						diff: {
							change_id: "change_workspace_approval",
							patch: expect.stringContaining("+approved content"),
						},
					},
				},
			]);
			expect(output.omitted_request).toMatchObject([
				{
					kind: "command.receipt",
					payload: {
						error: { code: "workspace.approval_conflict", retryable: false },
						status: "rejected",
					},
				},
			]);
			expect(output.changed_request).toMatchObject([
				{
					kind: "command.receipt",
					payload: {
						error: { code: "workspace.approval_conflict", retryable: false },
						status: "rejected",
					},
				},
			]);
			expect(output.approved).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						correlation_id: "workspace_approval_response",
						kind: "command.receipt",
						payload: expect.objectContaining({ status: "accepted" }),
					}),
					expect.objectContaining({
						kind: "event",
						payload: expect.objectContaining({
							approval: expect.objectContaining({ state: "applied" }),
							type: "workspace.replace.approval.updated",
						}),
					}),
				]),
			);
			expect(output.applied).toMatchObject([
				{
					correlation_id: "workspace_approval_applied",
					kind: "workspace.replace.approval.query.result",
					payload: { approval: { state: "applied" } },
				},
			]);
			expect(output.replace_replay).toMatchObject([
				{
					kind: "command.receipt",
					payload: {
						journal_sequence: output.request_sequence,
						status: "duplicate",
					},
				},
			]);
			expect(output.duplicate).toMatchObject([
				{
					correlation_id: "workspace_approval_response",
					kind: "command.receipt",
					payload: { status: "duplicate" },
				},
			]);
			expect(output.content).toBe("approved content");
		} finally {
			await runtime.dispose();
		}
	});

	it("reports corrupt immutable approval replay as a non-retryable invariant", async () => {
		const { root, runtime } = await make_workspace_runtime();

		try {
			await runtime.runPromise(seed_workspace_run(root));
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive(
							workspace_replace(
								"workspace_corrupt_approval_replace",
								"change_workspace_corrupt_approval",
								"corrupt approval content",
								content_identity("before"),
								"Approve before corruption.",
							),
						);
						const requested = yield* take_outbound(connection, 2);
						const approval_id = requested_approval_id(requested);
						const database = yield* Database;

						yield* database.client.run(
							`UPDATE journal_events SET payload_json = '{}' WHERE idempotency_key = 'workspace_replace_approval:${approval_id}:requested'`,
						);
						yield* connection.Receive(
							workspace_replace(
								"workspace_corrupt_approval_replace",
								"change_workspace_corrupt_approval",
								"corrupt approval content",
								content_identity("before"),
								"Approve before corruption.",
							),
						);

						return yield* take_outbound(connection, 1);
					}),
				),
			);

			expect(output).toMatchObject([
				{
					kind: "command.receipt",
					payload: {
						error: { code: "workspace.invariant_failed", retryable: false },
						status: "rejected",
					},
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("finishes committed approval evidence and private-payload cleanup before terminal state", async () => {
		const { database_path, root, runtime } = await make_workspace_runtime();
		const message_id = "workspace_committed_approval_cleanup";
		const change_id = "change_workspace_committed_approval_cleanup";
		let approval_id = "";

		try {
			await runtime.runPromise(seed_workspace_run(root));
			const setup = await runtime.runPromise(
				Effect.gen(function* () {
					const files = yield* WorkspaceFileService;
					const approvals = yield* WorkspaceReplaceApprovalRepository;
					const changes = yield* WorkspaceChangeRepository;
					const requested = yield* files.Replace({
						agent_id: "agent_workspace_protocol",
						approval_request: { reason: "Recover committed cleanup." },
						change_id,
						content: "committed cleanup content",
						expected_before: content_identity("before"),
						message_id,
						path: "src/example.ts",
						raw_origin: { provider: "codex", reference: "native_workspace_protocol" },
						run_id: "run_workspace_protocol",
						sent_at: workspace_time,
						thread_id: "thread_workspace_protocol",
						workspace_id: "workspace_protocol",
					});

					if (!("approval" in requested)) {
						return yield* Effect.die("Expected a workspace approval request");
					}

					yield* approvals.Decide({
						approval_id: requested.approval.approval_id,
						approved: true,
						message_id: "workspace_committed_approval_decision",
						sent_at: workspace_time,
						thread_id: "thread_workspace_protocol",
					});
					const execution = yield* approvals.ReadExecution(
						requested.approval.approval_id,
					);

					yield* changes.MarkApplied({
						_tag: "replace",
						message_id,
						result_identity: execution.approval.after_identity,
					});
					yield* changes.CommitRecorded(message_id, execution.prepared_diff);
					const database = yield* Database;
					const operations = yield* database.client
						.select()
						.from(WorkspaceChangeOperations);
					const operation = operations.find((row) => row.message_id === message_id);

					if (!operation) {
						return yield* Effect.die("Expected a committed workspace operation");
					}

					yield* database.client.run(
						`UPDATE workspace_change_operations SET request_fingerprint = '${"f".repeat(64)}' WHERE message_id = '${message_id}'`,
					);
					const corrupted_execution = yield* files
						.ExecuteApproved(requested.approval.approval_id)
						.pipe(Effect.exit);
					const evidence_events = (yield* database.client
						.select()
						.from(JournalEvents)).filter(
						(row) =>
							row.correlation_id === `workspace_evidence:${message_id}:correlation`,
					);

					yield* database.client.run(
						`UPDATE workspace_change_operations SET request_fingerprint = '${operation.request_fingerprint}' WHERE message_id = '${message_id}'`,
					);

					return {
						approval_id: requested.approval.approval_id,
						corrupted_execution,
						evidence_events,
					};
				}),
			);

			approval_id = setup.approval_id;
			expect(Exit.isFailure(setup.corrupted_execution)).toBe(true);
			expect(setup.evidence_events).toHaveLength(0);
		} finally {
			await runtime.dispose();
		}

		const recovered_runtime = make_workspace_backend_runtime(database_path, root);

		try {
			const recovered = await recovered_runtime.runPromise(
				Effect.gen(function* () {
					const approval = yield* WaitForApprovalState(approval_id, "applied");
					const database = yield* Database;
					const operations = yield* database.client
						.select()
						.from(WorkspaceChangeOperations);
					const payloads = yield* database.client
						.select()
						.from(WorkspaceMutationPayloads);
					const events = yield* database.client.select().from(JournalEvents);
					const operation = operations.find((row) => row.message_id === message_id);
					const payload = payloads.find((row) => row.message_id === message_id);
					const evidence_event = events.find(
						(row) =>
							row.correlation_id === `workspace_evidence:${message_id}:correlation`,
					);

					return { approval, evidence_event, operation, payload };
				}),
			);

			expect(recovered.approval.approval.state).toBe("applied");
			expect(recovered.operation?.evidence_recorded).toBe(true);
			expect(recovered.evidence_event).toMatchObject({
				agent_id: "agent_workspace_protocol",
				payload_json: JSON.stringify({
					operation: "write",
					path: "src/example.ts",
					type: "filesystem.mutation",
				}),
				raw_origin_json: JSON.stringify({
					provider: "codex",
					reference: "native_workspace_protocol",
				}),
				run_id: "run_workspace_protocol",
				thread_id: "thread_workspace_protocol",
			});
			expect(recovered.payload).toMatchObject({
				expected: null,
				replacement: null,
				state: "consumed",
			});
		} finally {
			await recovered_runtime.dispose();
		}
	});

	it("finishes rejected approval private-state cleanup before terminal state", async () => {
		const { database_path, root, runtime } = await make_workspace_runtime();
		const message_id = "workspace_rejected_approval_cleanup";
		const change_id = "change_workspace_rejected_approval_cleanup";
		let approval_id = "";

		try {
			await runtime.runPromise(seed_workspace_run(root));
			approval_id = await runtime.runPromise(
				Effect.gen(function* () {
					const files = yield* WorkspaceFileService;
					const approvals = yield* WorkspaceReplaceApprovalRepository;
					const changes = yield* WorkspaceChangeRepository;
					const requested = yield* files.Replace({
						agent_id: "agent_workspace_protocol",
						approval_request: { reason: "Recover rejected cleanup." },
						change_id,
						content: "rejected cleanup content",
						expected_before: content_identity("before"),
						message_id,
						path: "src/example.ts",
						raw_origin: { provider: "codex", reference: "native_workspace_protocol" },
						run_id: "run_workspace_protocol",
						sent_at: workspace_time,
						thread_id: "thread_workspace_protocol",
						workspace_id: "workspace_protocol",
					});

					if (!("approval" in requested)) {
						return yield* Effect.die("Expected a workspace approval request");
					}

					yield* approvals.Decide({
						approval_id: requested.approval.approval_id,
						approved: true,
						message_id: "workspace_rejected_approval_decision",
						sent_at: workspace_time,
						thread_id: "thread_workspace_protocol",
					});
					yield* changes.RejectChanged(message_id);

					return requested.approval.approval_id;
				}),
			);
		} finally {
			await runtime.dispose();
		}

		const recovered_runtime = make_workspace_backend_runtime(database_path, root);

		try {
			const recovered = await recovered_runtime.runPromise(
				Effect.gen(function* () {
					const approval = yield* WaitForApprovalState(approval_id, "rejected");
					const database = yield* Database;
					const payloads = yield* database.client
						.select()
						.from(WorkspaceMutationPayloads);
					const snapshots = yield* database.client
						.select()
						.from(WorkspaceChangeSnapshots);
					const payload = payloads.find((row) => row.message_id === message_id);
					const snapshot = snapshots.find((row) => row.change_id === change_id);

					return { approval, payload, snapshot };
				}),
			);

			expect(recovered.approval.approval.state).toBe("rejected");
			expect(recovered.payload).toMatchObject({
				expected: null,
				replacement: null,
				state: "consumed",
			});
			expect(recovered.snapshot).toMatchObject({ content: null, state: "consumed" });
		} finally {
			await recovered_runtime.dispose();
		}
	});

	it("recovers approved execution and denied cleanup after a coordinator restart", async () => {
		const { database_path, root, runtime } = await make_workspace_runtime();
		let approved_id = "";
		let approved_request_sequence = 0;
		let denied_id = "";
		let denied_request_sequence = 0;

		try {
			await runtime.runPromise(seed_workspace_run(root));
			await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive(
							workspace_replace(
								"workspace_restart_approved_replace",
								"change_workspace_restart_approved",
								"recovered content",
								content_identity("before"),
								"Approve after restart.",
							),
						);
						const approved_request = yield* take_outbound(connection, 2);

						approved_id = requested_approval_id(approved_request);
						approved_request_sequence = command_receipt_sequence(approved_request);

						yield* connection.Receive(
							workspace_replace(
								"workspace_restart_denied_replace",
								"change_workspace_restart_denied",
								"denied content",
								content_identity("before"),
								"Deny after restart.",
							),
						);
						const denied_request = yield* take_outbound(connection, 2);

						denied_id = requested_approval_id(denied_request);
						denied_request_sequence = command_receipt_sequence(denied_request);

						const coordinator = yield* WorkspaceReplaceApprovalCoordinator;

						yield* coordinator.QuiesceThread("thread_workspace_protocol");
						yield* connection.Receive(
							workspace_approval_response(
								"workspace_restart_approved_response",
								approved_id,
								true,
							),
						);
						yield* take_outbound(connection, 2);
						yield* connection.Receive(
							workspace_approval_response(
								"workspace_restart_denied_response",
								denied_id,
								false,
							),
						);
						yield* take_outbound(connection, 2);
						yield* WaitForApprovalState(approved_id, "approved");
						yield* WaitForApprovalState(denied_id, "denied");
					}),
				),
			);
		} finally {
			await runtime.dispose();
		}

		const recovered_runtime = make_workspace_backend_runtime(database_path, root);

		try {
			const recovered = await recovered_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const approval = yield* WaitForApprovalState(approved_id, "applied");

						yield* WaitForDeniedSettlement(denied_id);

						const denial = yield* WaitForApprovalState(denied_id, "denied");
						const content = yield* Effect.promise(() =>
							readFile(join(root, "src", "example.ts"), "utf8"),
						);
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive(
							workspace_replace(
								"workspace_restart_approved_replace",
								"change_workspace_restart_approved",
								"recovered content",
								content_identity("before"),
								"Approve after restart.",
							),
						);
						const approved_replay = yield* take_outbound(connection, 1);

						yield* connection.Receive(
							workspace_replace(
								"workspace_restart_denied_replace",
								"change_workspace_restart_denied",
								"denied content",
								content_identity("before"),
								"Deny after restart.",
							),
						);
						const denied_replay = yield* take_outbound(connection, 1);

						return { approval, approved_replay, content, denial, denied_replay };
					}),
				),
			);

			expect(recovered.approval.approval.state).toBe("applied");
			expect(recovered.denial.approval.state).toBe("denied");
			expect(recovered.content).toBe("recovered content");
			expect(recovered.approved_replay).toMatchObject([
				{
					kind: "command.receipt",
					payload: {
						journal_sequence: approved_request_sequence,
						status: "duplicate",
					},
				},
			]);
			expect(recovered.denied_replay).toMatchObject([
				{
					kind: "command.receipt",
					payload: {
						journal_sequence: denied_request_sequence,
						status: "duplicate",
					},
				},
			]);
		} finally {
			await recovered_runtime.dispose();
		}
	});

	it("rejects traffic before hello and versionless unsupported handshakes", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const before_hello_connection = yield* open_connection;

						yield* before_hello_connection.Receive(make_command());

						const before_hello = yield* take_outbound(before_hello_connection, 1);
						const unsupported_connection = yield* open_connection;

						yield* unsupported_connection.Receive(
							make_hello("hello_unsupported", 0, [], [2]),
						);

						const unsupported = yield* take_outbound(unsupported_connection, 1);

						return { before_hello, unsupported };
					}),
				),
			);

			expect(output.before_hello).toMatchObject([
				{
					correlation_id: "command_1",
					kind: "protocol.error",
					payload: { code: "protocol.handshake_required", retryable: false },
				},
			]);
			expect(output.unsupported).toMatchObject([
				{
					correlation_id: "hello_unsupported",
					kind: "protocol.error",
					payload: { code: "protocol.unsupported_version", retryable: false },
				},
			]);
			expect(output.unsupported[0]).not.toHaveProperty("protocol_version");
		} finally {
			await runtime.dispose();
		}
	});

	it("negotiates, returns a query snapshot, and validates acknowledgements", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;
						const handshake = yield* negotiate(connection);

						yield* connection.Receive(make_command());

						const command = yield* take_outbound(connection, 2);

						yield* connection.Receive(make_query());

						const query = yield* take_outbound(connection, 1);

						yield* connection.Receive(
							make_ack("ack_valid", 2, [
								{ sequence: 1, stream_id: "settings:guidance" },
								{ sequence: 1, stream_id: "thread:thread_1" },
							]),
						);
						yield* connection.Receive(
							make_ack("ack_duplicate_cursor", 2, [
								{ sequence: 1, stream_id: "thread:thread_1" },
								{ sequence: 1, stream_id: "thread:thread_1" },
							]),
						);
						yield* connection.Receive(
							make_ack("ack_projection_cursor", 2, [
								{
									sequence: 1,
									stream_id: "projection:thread.list:subscription_1",
								},
							]),
						);
						yield* connection.Receive(
							make_ack("ack_out_of_range", 3, [
								{ sequence: 2, stream_id: "thread:thread_1" },
							]),
						);

						const acknowledgement = yield* take_outbound(connection, 3);

						return { acknowledgement, command, handshake, query };
					}),
				),
			);

			expect(output.handshake.map((envelope) => envelope.kind)).toEqual([
				"welcome",
				"event",
				"replay.complete",
			]);
			expect(output.handshake[1]).toMatchObject({
				journal_sequence: 1,
				payload: { type: "guidance.canonical.updated" },
				stream_id: "settings:guidance",
			});
			expect(output.command).toMatchObject([
				{
					correlation_id: "command_1",
					kind: "command.receipt",
					payload: { journal_sequence: 2, status: "accepted" },
				},
				{
					correlation_id: "command_1",
					journal_sequence: 2,
					kind: "event",
				},
			]);
			expect(output.query).toMatchObject([
				{
					correlation_id: "query_1",
					kind: "thread.list.query.result",
					payload: {
						journal_sequence: 2,
						threads: [{ thread_id: "thread_1", title: "Protocol integration" }],
					},
				},
			]);
			expect(
				output.acknowledgement.map((envelope) =>
					"correlation_id" in envelope ? envelope.correlation_id : undefined,
				),
			).toEqual(["ack_duplicate_cursor", "ack_projection_cursor", "ack_out_of_range"]);
			expect(output.acknowledgement).toMatchObject(
				Array.from({ length: 3 }, () => ({
					kind: "protocol.error",
					payload: { code: "protocol.invalid_ack", retryable: false },
				})),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("sends subscription snapshots and does not replay duplicate commands live", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive(make_subscribe());

						const subscription = yield* take_outbound(connection, 2);

						yield* connection.Receive(make_command());

						const accepted = yield* take_outbound(connection, 3);
						const duplicate_delivery = yield* take_until_outbound(
							connection,
							(envelope) => envelope.kind === "thread.list.query.result",
						).pipe(Effect.forkChild);

						yield* connection.Receive(make_command());
						yield* connection.Receive(make_query("query_after_duplicate"));

						const after_duplicate = yield* Fiber.join(duplicate_delivery);

						return { accepted, after_duplicate, subscription };
					}),
				),
			);

			expect(output.subscription).toMatchObject([
				{
					correlation_id: "subscribe_1",
					kind: "subscription.started",
					subscription_id: "subscription_1",
				},
				{
					kind: "thread.list.snapshot",
					payload: { threads: [] },
					sequence: 0,
					subscription_id: "subscription_1",
				},
			]);
			expect(output.accepted).toMatchObject([
				{
					kind: "command.receipt",
					payload: { status: "accepted" },
				},
				{ kind: "event", journal_sequence: 2 },
				{
					kind: "thread.list.upsert",
					payload: { thread_id: "thread_1", title: "Protocol integration" },
					sequence: 1,
				},
			]);
			expect(output.after_duplicate.map((envelope) => envelope.kind)).toEqual([
				"command.receipt",
				"thread.list.query.result",
			]);
			expect(output.after_duplicate).toMatchObject([
				{
					correlation_id: "command_1",
					kind: "command.receipt",
					payload: { status: "duplicate" },
				},
				{
					correlation_id: "query_after_duplicate",
					kind: "thread.list.query.result",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("reconnects from a cursor and replays only missing events", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const replay = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const initial_connection = yield* open_connection;

						yield* negotiate(initial_connection);
						yield* initial_connection.Receive(make_command());
						yield* take_outbound(initial_connection, 2);
						yield* initial_connection.Close;

						const producer_connection = yield* open_connection;

						yield* negotiate(producer_connection, make_hello("hello_producer"));
						yield* producer_connection.Receive(
							make_command("command_2", "thread_2", "Missing event"),
						);
						yield* take_outbound(producer_connection, 2);

						const reconnecting_connection = yield* open_connection;

						yield* reconnecting_connection.Receive(
							make_hello("hello_reconnect", 2, [
								{ sequence: 1, stream_id: "settings:guidance" },
								{ sequence: 1, stream_id: "thread:thread_1" },
							]),
						);

						return yield* take_outbound(reconnecting_connection, 3);
					}),
				),
			);

			expect(replay.map((envelope) => envelope.kind)).toEqual([
				"welcome",
				"event",
				"replay.complete",
			]);
			expect(replay[1]).toMatchObject({
				journal_sequence: 3,
				payload: { title: "Missing event", type: "thread.created" },
				thread_id: "thread_2",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("broadcasts committed events to two live connections", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const observer_event = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const source_connection = yield* open_connection;
						const observer_connection = yield* open_connection;

						yield* negotiate(source_connection, make_hello("hello_source"));
						yield* negotiate(observer_connection, make_hello("hello_observer"));
						yield* source_connection.Receive(make_command());
						yield* take_outbound(source_connection, 2);

						return yield* take_outbound(observer_connection, 1);
					}),
				),
			);

			expect(observer_event).toMatchObject([
				{
					correlation_id: "command_1",
					journal_sequence: 2,
					kind: "event",
					thread_id: "thread_1",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects malformed negotiated input and closes connections with their scope", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const malformed = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive({
							kind: "command",
							message_id: "malformed_command",
							origin: "frontend",
							protocol_version: 1,
							schema_version: 1,
							sent_at: "not-an-iso-date",
						});

						return yield* take_outbound(connection, 1);
					}),
				),
			);
			const closed_connection = await runtime.runPromise(Effect.scoped(open_connection));

			await runtime.runPromise(closed_connection.Closed);

			expect(malformed).toMatchObject([
				{
					kind: "protocol.error",
					payload: { code: "protocol.invalid_message", retryable: false },
					protocol_version: 1,
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("validates connection settings before constructing the server", async () => {
		const invalid_capacity = await Effect.runPromise(
			DecodeProtocolConnectionOptions({
				heartbeat_interval_ms: 10,
				heartbeat_timeout_ms: 20,
				outbound_capacity: 0,
			}).pipe(Effect.flip),
		);
		const invalid_timeout = await Effect.runPromise(
			DecodeProtocolConnectionOptions({
				heartbeat_interval_ms: 20,
				heartbeat_timeout_ms: 10,
				outbound_capacity: 1,
			}).pipe(Effect.flip),
		);

		expect(invalid_capacity._tag).toBe("ProtocolConfigurationError");
		expect(invalid_timeout._tag).toBe("ProtocolConfigurationError");
	});
});

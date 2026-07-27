import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	GitService,
	GitServiceError,
	make_backend_runtime,
	ProtocolServer,
	type GitMutationAcceptance,
	type ProtocolConnection,
} from "@artisan/backend";
import type {
	EventEnvelope,
	GitDiffQueryEnvelope,
	GitDiffQueryResult,
	GitIndexStageRequestEnvelope,
	GitIndexUnstageRequestEnvelope,
	GitMutationProjection,
	GitMutationResolveEnvelope,
	GitWorkspaceQueryEnvelope,
	GitWorkspaceQueryResult,
	HelloEnvelope,
	OutboundControlEnvelope,
	ThreadCreateEnvelope,
} from "@artisan/protocol";

import { JournalStore } from "../../modules/backend/src/persistence/journal-store";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const sent_at = "2026-07-18T12:00:00.000Z";
const snapshot_id = "a".repeat(64);

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-editor-git-protocol-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_hello(): HelloEnvelope {
	return {
		kind: "hello",
		message_id: "hello_git_protocol",
		origin: "frontend",
		payload: {
			event_cursors: [],
			last_journal_sequence: 0,
			supported_protocol_versions: [1],
		},
		schema_version: 1,
		sent_at,
	};
}

function thread_create(): ThreadCreateEnvelope {
	return {
		kind: "thread.create.request",
		message_id: "create_git_protocol_thread",
		origin: "frontend",
		payload: { title: "Git protocol" },
		protocol_version: 1,
		schema_version: 1,
		sent_at,
	};
}

function workspace_query(
	message_id = "git_workspace_query",
	workspace_id = "workspace_git_protocol",
	thread_id = "thread_git_protocol",
): GitWorkspaceQueryEnvelope {
	return {
		kind: "git.workspace.query",
		message_id,
		origin: "frontend",
		payload: { thread_id, workspace_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at,
	};
}

function diff_query(
	message_id = "git_diff_query",
	expected_snapshot_id = snapshot_id,
): GitDiffQueryEnvelope {
	return {
		kind: "git.diff.query",
		message_id,
		origin: "frontend",
		payload: {
			expected_snapshot_id,
			expected_workspace_version: 1,
			max_bytes: 4_096,
			scope: "aggregate",
			workspace_id: "workspace_git_protocol",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at,
	};
}

function stage_request(
	message_id = "git_stage_request",
	path = "src/example.ts",
	thread_id = "thread_git_protocol",
): GitIndexStageRequestEnvelope {
	return {
		agent_id: "agent_git_protocol",
		kind: "git.index.stage.request",
		message_id,
		origin: "frontend",
		payload: {
			approval_id: "approval_stage",
			expected_snapshot_id: snapshot_id,
			expected_workspace_version: 1,
			mutation_id: "mutation_stage",
			paths: [path],
			workspace_id: "workspace_git_protocol",
		},
		protocol_version: 1,
		raw_origin: { provider: "codex", reference: "git_protocol_stage" },
		run_id: "run_git_protocol",
		schema_version: 1,
		sent_at,
		thread_id,
	};
}

function unstage_request(thread_id = "thread_git_protocol"): GitIndexUnstageRequestEnvelope {
	return {
		agent_id: "agent_git_protocol",
		kind: "git.index.unstage.request",
		message_id: "git_unstage_request",
		origin: "frontend",
		payload: {
			approval_id: "approval_unstage",
			expected_snapshot_id: snapshot_id,
			expected_workspace_version: 1,
			mutation_id: "mutation_unstage",
			paths: ["src/staged.ts"],
			workspace_id: "workspace_git_protocol",
		},
		protocol_version: 1,
		run_id: "run_git_protocol",
		schema_version: 1,
		sent_at,
		thread_id,
	};
}

function resolve_request(thread_id = "thread_git_protocol"): GitMutationResolveEnvelope {
	return {
		agent_id: "agent_reviewer",
		kind: "git.mutation.resolve",
		message_id: "git_resolve_request",
		origin: "frontend",
		payload: {
			approval_id: "approval_stage",
			approved: true,
			mutation_id: "mutation_stage",
		},
		protocol_version: 1,
		run_id: "run_reviewer",
		schema_version: 1,
		sent_at,
		thread_id,
	};
}

type GitMutationRequestEnvelope = GitIndexStageRequestEnvelope | GitIndexUnstageRequestEnvelope;

type GitMutationEnvelope = GitMutationRequestEnvelope | GitMutationResolveEnvelope;

function make_fake_git_service() {
	const queries: Array<GitWorkspaceQueryEnvelope> = [];
	const diffs: Array<GitDiffQueryEnvelope> = [];
	const requests: Array<GitMutationRequestEnvelope> = [];
	const resolutions: Array<GitMutationResolveEnvelope> = [];
	const acceptances_by_message = new Map<string, GitMutationAcceptance>();
	const mutations = new Map<string, GitMutationProjection>();
	let journal: typeof JournalStore.Service | undefined;

	const append_event = (envelope: GitMutationEnvelope, payload: EventEnvelope["payload"]) => {
		if (journal === undefined) {
			return Effect.die("The fake Git journal is not bound");
		}

		return journal.AppendEvent({
			...(envelope.agent_id === undefined ? {} : { agent_id: envelope.agent_id }),
			causation_id: envelope.message_id,
			correlation_id: envelope.message_id,
			payload,
			...(envelope.raw_origin === undefined ? {} : { raw_origin: envelope.raw_origin }),
			...(envelope.run_id === undefined ? {} : { run_id: envelope.run_id }),
			thread_id: envelope.thread_id,
		});
	};

	const accept_request = (envelope: GitMutationRequestEnvelope) =>
		Effect.gen(function* () {
			const duplicate = acceptances_by_message.get(envelope.message_id);

			if (duplicate !== undefined) {
				return { ...duplicate, status: "duplicate" as const };
			}

			const current_journal = journal;

			if (current_journal === undefined) {
				return yield* Effect.die("The fake Git journal is not bound");
			}

			const journal_sequence = (yield* current_journal.ReadWatermark()) + 1;
			const mutation: GitMutationProjection = {
				...(envelope.agent_id === undefined ? {} : { agent_id: envelope.agent_id }),
				approval_id: envelope.payload.approval_id,
				expected_snapshot_id: envelope.payload.expected_snapshot_id,
				expected_workspace_version: envelope.payload.expected_workspace_version,
				journal_sequence,
				kind: envelope.kind === "git.index.stage.request" ? "stage" : "unstage",
				lifecycle: "awaiting_approval",
				mutation_id: envelope.payload.mutation_id,
				paths: envelope.payload.paths,
				...(envelope.raw_origin === undefined ? {} : { raw_origin: envelope.raw_origin }),
				requested_at: envelope.sent_at,
				...(envelope.run_id === undefined ? {} : { run_id: envelope.run_id }),
				source_message_id: envelope.message_id,
				thread_id: envelope.thread_id,
				updated_at: sent_at,
				workspace_id: envelope.payload.workspace_id,
			};
			const event = yield* append_event(envelope, {
				mutation,
				type: "git.mutation.updated",
			});
			const acceptance: GitMutationAcceptance = {
				event,
				mutation,
				status: "accepted",
			};

			acceptances_by_message.set(envelope.message_id, acceptance);
			mutations.set(mutation.mutation_id, mutation);

			return acceptance;
		}).pipe(
			Effect.mapError(
				(cause) =>
					new GitServiceError({
						operation: "request",
						reason: "unavailable",
						retryable: true,
						cause,
					}),
			),
		);

	const accept_resolution = (envelope: GitMutationResolveEnvelope) =>
		Effect.gen(function* () {
			const duplicate = acceptances_by_message.get(envelope.message_id);

			if (duplicate !== undefined) {
				return { ...duplicate, status: "duplicate" as const };
			}

			const existing = mutations.get(envelope.payload.mutation_id);

			if (existing === undefined) {
				return yield* Effect.fail(
					new GitServiceError({
						operation: "resolve",
						reason: "unavailable",
						retryable: false,
					}),
				);
			}

			yield* append_event(envelope, {
				changed_file_count: 1,
				has_diff: true,
				root_path: "C:/workspace",
				type: "git.workspace.observed",
				worktree_path: "C:/workspace",
			});

			const current_journal = journal;

			if (current_journal === undefined) {
				return yield* Effect.die("The fake Git journal is not bound");
			}

			const journal_sequence = (yield* current_journal.ReadWatermark()) + 1;
			const mutation: GitMutationProjection = {
				...existing,
				decision_at: envelope.sent_at,
				decision_message_id: envelope.message_id,
				journal_sequence,
				lifecycle: "approved",
				updated_at: sent_at,
			};
			const event = yield* append_event(envelope, {
				mutation,
				type: "git.mutation.updated",
			});
			yield* append_event(envelope, {
				source: "git",
				type: "process.ownership",
				working_directory: "C:/workspace",
			});
			const acceptance: GitMutationAcceptance = {
				event,
				mutation,
				status: "accepted",
			};

			acceptances_by_message.set(envelope.message_id, acceptance);
			mutations.set(mutation.mutation_id, mutation);

			return acceptance;
		}).pipe(
			Effect.mapError((cause) =>
				cause instanceof GitServiceError
					? cause
					: new GitServiceError({
							operation: "resolve",
							reason: "unavailable",
							retryable: true,
							cause,
						}),
			),
		);

	return {
		bind_journal: (service: typeof JournalStore.Service) => {
			journal = service;
		},
		diffs,
		layer: Layer.succeed(GitService, {
			Diff: (envelope) => {
				diffs.push(envelope);

				return envelope.payload.expected_snapshot_id === "c".repeat(64)
					? Effect.fail(
							new GitServiceError({
								cause: new Error("PRIVATE DIFF FAILURE"),
								operation: "diff",
								reason: "changed",
								retryable: false,
							}),
						)
					: Effect.succeed({
							byte_count: 4,
							format: "unified",
							format_version: 1,
							patch: "diff",
							scope: envelope.payload.scope,
							snapshot_id: envelope.payload.expected_snapshot_id,
							truncated: false,
							workspace_id: envelope.payload.workspace_id,
							workspace_version: envelope.payload.expected_workspace_version,
						} satisfies GitDiffQueryResult);
			},
			Query: (envelope) => {
				queries.push(envelope);

				return envelope.payload.workspace_id === "workspace_unavailable"
					? Effect.fail(
							new GitServiceError({
								cause: new Error("PRIVATE QUERY FAILURE"),
								operation: "query",
								reason: "unavailable",
								retryable: true,
							}),
						)
					: Effect.succeed({
							journal_sequence: 0,
							pending_mutations: [],
							workspace: {
								journal_sequence: 0,
								observed_at: sent_at,
								repository_state: "not_repository",
								snapshot_id,
								version: 1,
								workspace_id: envelope.payload.workspace_id,
							},
						} satisfies GitWorkspaceQueryResult);
			},
			Request: (envelope) => {
				requests.push(envelope);

				return envelope.payload.paths.includes("invalid.ts")
					? Effect.fail(
							new GitServiceError({
								cause: new Error("PRIVATE PATH FAILURE"),
								operation: "request",
								reason: "invalid_path",
								retryable: false,
							}),
						)
					: accept_request(envelope);
			},
			Resolve: (envelope) => {
				resolutions.push(envelope);

				return accept_resolution(envelope);
			},
		}),
		queries,
		requests,
		resolutions,
	};
}

const open_connection = Effect.gen(function* () {
	const server = yield* ProtocolServer;

	return yield* server.Open;
});

function take_outbound(connection: ProtocolConnection, count: number) {
	return connection.Outbound.pipe(Stream.take(count), Stream.runCollect);
}

const negotiate = (connection: ProtocolConnection) =>
	Effect.gen(function* () {
		yield* connection.Receive(make_hello());

		return yield* connection.Outbound.pipe(
			Stream.takeUntil((envelope) => envelope.kind === "replay.complete"),
			Stream.runCollect,
		);
	});

function to_array(output: Iterable<OutboundControlEnvelope>) {
	return Array.from(output);
}

function created_thread_id(output: Iterable<OutboundControlEnvelope>) {
	const result = [...output].find((envelope) => envelope.kind === "thread.create.result");

	if (result?.kind !== "thread.create.result") {
		throw new Error("Forge did not return the created Git protocol thread");
	}

	return result.payload.thread_id;
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("Git protocol server", () => {
	it("routes every Git V1 envelope with correlated results, receipts, and live events", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_git_service();
		const runtime = make_backend_runtime({
			database_path,
			git_service: fake.layer,
			migrations_path,
		});

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;
						const journal = yield* JournalStore;

						fake.bind_journal(journal);
						yield* negotiate(connection);
						yield* connection.Receive(thread_create());
						const creation = yield* take_outbound(connection, 2);
						const thread_id = created_thread_id(creation);

						const query = workspace_query(
							"git_workspace_query",
							"workspace_git_protocol",
							thread_id,
						);
						yield* connection.Receive(query);
						const query_output = yield* take_outbound(connection, 1);

						const diff = diff_query();
						yield* connection.Receive(diff);
						const diff_output = yield* take_outbound(connection, 1);

						const stage = stage_request(
							"git_stage_request",
							"src/example.ts",
							thread_id,
						);
						yield* connection.Receive(stage);
						const stage_output = yield* take_outbound(connection, 2);

						yield* connection.Receive(stage);
						const duplicate_output = yield* take_outbound(connection, 1);

						const query_after_duplicate = workspace_query(
							"query_after_duplicate",
							"workspace_git_protocol",
							thread_id,
						);
						yield* connection.Receive(query_after_duplicate);
						const after_duplicate_output = yield* take_outbound(connection, 1);

						const unstage = unstage_request(thread_id);
						yield* connection.Receive(unstage);
						const unstage_output = yield* take_outbound(connection, 2);

						const resolve = resolve_request(thread_id);
						yield* connection.Receive(resolve);
						const resolve_output = yield* take_outbound(connection, 4);

						return {
							after_duplicate_output,
							diff_output,
							duplicate_output,
							query_output,
							resolve_output,
							stage_output,
							thread_id,
							unstage_output,
						};
					}),
				),
			);

			const [query_result] = to_array(output.query_output);
			const [diff_result] = to_array(output.diff_output);
			const [stage_receipt, stage_event] = to_array(output.stage_output);
			const [duplicate_receipt] = to_array(output.duplicate_output);
			const [after_duplicate_result] = to_array(output.after_duplicate_output);
			const [unstage_receipt, unstage_event] = to_array(output.unstage_output);
			const [resolve_receipt, preceding_event, resolve_event, trailing_event] = to_array(
				output.resolve_output,
			);
			const stage_sequence =
				stage_receipt?.kind === "command.receipt" &&
				stage_receipt.payload.status !== "rejected"
					? stage_receipt.payload.journal_sequence
					: undefined;

			expect(query_result).toMatchObject({
				correlation_id: "git_workspace_query",
				kind: "git.workspace.query.result",
				origin: "backend",
				payload: { workspace: { workspace_id: "workspace_git_protocol" } },
			});
			expect(diff_result).toMatchObject({
				correlation_id: "git_diff_query",
				kind: "git.diff.query.result",
				payload: { patch: "diff", snapshot_id },
			});
			expect(stage_receipt).toMatchObject({
				agent_id: "agent_git_protocol",
				causation_id: "git_stage_request",
				correlation_id: "git_stage_request",
				kind: "command.receipt",
				payload: { status: "accepted" },
				run_id: "run_git_protocol",
			});
			expect(stage_event).toMatchObject({
				journal_sequence: stage_sequence,
				kind: "event",
				payload: {
					mutation: { kind: "stage", lifecycle: "awaiting_approval" },
					type: "git.mutation.updated",
				},
			});
			expect(duplicate_receipt).toMatchObject({
				kind: "command.receipt",
				payload: { journal_sequence: stage_sequence, status: "duplicate" },
			});
			expect(after_duplicate_result).toMatchObject({
				correlation_id: "query_after_duplicate",
				kind: "git.workspace.query.result",
			});
			expect(unstage_receipt).toMatchObject({
				kind: "command.receipt",
				payload: { journal_sequence: (stage_sequence ?? 0) + 1, status: "accepted" },
			});
			expect(unstage_event).toMatchObject({
				journal_sequence: (stage_sequence ?? 0) + 1,
				payload: { mutation: { kind: "unstage" }, type: "git.mutation.updated" },
			});
			expect(resolve_receipt).toMatchObject({
				agent_id: "agent_reviewer",
				kind: "command.receipt",
				payload: { journal_sequence: (stage_sequence ?? 0) + 3, status: "accepted" },
				run_id: "run_reviewer",
			});
			expect(preceding_event).toMatchObject({
				journal_sequence: (stage_sequence ?? 0) + 2,
				kind: "event",
				payload: { type: "git.workspace.observed" },
			});
			expect(resolve_event).toMatchObject({
				journal_sequence: (stage_sequence ?? 0) + 3,
				payload: { mutation: { lifecycle: "approved" }, type: "git.mutation.updated" },
			});
			expect(trailing_event).toMatchObject({
				journal_sequence: (stage_sequence ?? 0) + 4,
				kind: "event",
				payload: { type: "process.ownership" },
			});
			expect(fake.queries).toEqual([
				workspace_query("git_workspace_query", "workspace_git_protocol", output.thread_id),
				workspace_query(
					"query_after_duplicate",
					"workspace_git_protocol",
					output.thread_id,
				),
			]);
			expect(fake.diffs).toEqual([diff_query()]);
			expect(fake.requests).toEqual([
				stage_request("git_stage_request", "src/example.ts", output.thread_id),
				stage_request("git_stage_request", "src/example.ts", output.thread_id),
				unstage_request(output.thread_id),
			]);
			expect(fake.resolutions).toEqual([resolve_request(output.thread_id)]);
		} finally {
			await runtime.dispose();
		}
	});

	it("maps Git failures to correlated sanitized protocol errors and rejected receipts", async () => {
		const database_path = await make_database_path();
		const fake = make_fake_git_service();
		const runtime = make_backend_runtime({
			database_path,
			git_service: fake.layer,
			migrations_path,
		});

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive(thread_create());
						const creation = yield* take_outbound(connection, 2);
						const thread_id = created_thread_id(creation);

						yield* connection.Receive(
							workspace_query(
								"git_query_failure",
								"workspace_unavailable",
								thread_id,
							),
						);
						const query_error = yield* take_outbound(connection, 1);

						yield* connection.Receive(diff_query("git_diff_failure", "c".repeat(64)));
						const diff_error = yield* take_outbound(connection, 1);

						yield* connection.Receive(
							stage_request("git_stage_failure", "invalid.ts", thread_id),
						);
						const mutation_error = yield* take_outbound(connection, 1);

						return { diff_error, mutation_error, query_error };
					}),
				),
			);

			const [query_error] = to_array(output.query_error);
			const [diff_error] = to_array(output.diff_error);
			const [mutation_error] = to_array(output.mutation_error);

			expect(query_error).toMatchObject({
				correlation_id: "git_query_failure",
				kind: "protocol.error",
				payload: { code: "git.unavailable", retryable: true },
			});
			expect(diff_error).toMatchObject({
				correlation_id: "git_diff_failure",
				kind: "protocol.error",
				payload: { code: "git.changed", retryable: false },
			});
			expect(mutation_error).toMatchObject({
				agent_id: "agent_git_protocol",
				causation_id: "git_stage_failure",
				correlation_id: "git_stage_failure",
				kind: "command.receipt",
				payload: {
					error: { code: "git.invalid_path", retryable: false },
					status: "rejected",
				},
				run_id: "run_git_protocol",
			});
			expect(JSON.stringify(output)).not.toContain("PRIVATE");
		} finally {
			await runtime.dispose();
		}
	});
});

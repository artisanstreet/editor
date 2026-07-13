import { execFile } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { promisify } from "node:util";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import type {
	HelloEnvelope,
	OutboundControlEnvelope,
	WorkspaceGitCheckoutApprovalQueryEnvelope,
	WorkspaceGitCheckoutApprovalRespondEnvelope,
	WorkspaceGitCheckoutRequestEnvelope,
	WorkspaceGitSessionQueryEnvelope,
	WorkspaceGitSessionRefreshEnvelope,
} from "@artisan/protocol";
import {
	make_backend_runtime,
	ProtocolServer,
	ProtocolRouter,
	type ProtocolConnection,
	make_node_workspace_git_registry_layer,
} from "@artisan/backend";
import {
	WorkspaceGitRegistrationError,
	WorkspaceGitRegistry,
} from "../../modules/backend/src/git/workspace-git-registry";
import { Effect, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

const exec_file = promisify(execFile);
const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const protocol_time = "2026-07-13T12:00:00.000Z";
const workspace_id = "workspace_git_protocol";
const thread_id = "thread_git_protocol";

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

function make_hello(
	message_id = "hello_1",
	last_journal_sequence = 0,
	event_cursors: HelloEnvelope["payload"]["event_cursors"] = [],
): HelloEnvelope {
	return {
		kind: "hello",
		message_id,
		origin: "frontend",
		payload: {
			event_cursors,
			last_journal_sequence,
			supported_protocol_versions: [1],
		},
		schema_version: 1,
		sent_at: protocol_time,
	};
}

const negotiate = (connection: ProtocolConnection, hello = make_hello()) =>
	Effect.gen(function* () {
		yield* connection.Receive(hello);

		return yield* take_until_outbound(
			connection,
			(envelope) => envelope.kind === "replay.complete",
		);
	});

function session_query(
	message_id: string,
	target_workspace_id = workspace_id,
): WorkspaceGitSessionQueryEnvelope {
	return {
		kind: "workspace.git.session.query",
		message_id,
		origin: "frontend",
		payload: { workspace_id: target_workspace_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
	};
}

function session_refresh(message_id: string): WorkspaceGitSessionRefreshEnvelope {
	return {
		kind: "workspace.git.session.refresh",
		message_id,
		origin: "frontend",
		payload: { workspace_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
		thread_id,
	};
}

function checkout_request(
	message_id: string,
	expected_session_version: number,
	target_branch = "feature/git-session",
): WorkspaceGitCheckoutRequestEnvelope {
	return {
		kind: "workspace.git.checkout.request",
		message_id,
		origin: "frontend",
		payload: { expected_session_version, target_branch, workspace_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
		thread_id,
	};
}

function approval_query(
	message_id: string,
	approval_id: string,
): WorkspaceGitCheckoutApprovalQueryEnvelope {
	return {
		kind: "workspace.git.checkout.approval.query",
		message_id,
		origin: "frontend",
		payload: { approval_id, thread_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
	};
}

function approval_response(
	message_id: string,
	approval_id: string,
	approved: boolean,
): WorkspaceGitCheckoutApprovalRespondEnvelope {
	return {
		kind: "workspace.git.checkout.approval.respond",
		message_id,
		origin: "frontend",
		payload: { approval_id, approved },
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
		thread_id,
	};
}

async function make_repository() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-git-protocol-"));
	const root = join(directory, "repository");

	temporary_directories.push(directory);
	await mkdir(root, { recursive: true });
	await exec_file("git", ["init", "-b", "main"], { cwd: root });
	await exec_file("git", ["config", "user.email", "protocol@example.test"], { cwd: root });
	await exec_file("git", ["config", "user.name", "Protocol Test"], { cwd: root });
	await writeFile(join(root, "accepted.txt"), "main\n");
	await exec_file("git", ["add", "accepted.txt"], { cwd: root });
	await exec_file("git", ["commit", "-m", "initial"], { cwd: root });
	await exec_file("git", ["branch", "feature/git-session"], { cwd: root });

	return { root, database_path: join(directory, "artisan.db") };
}

function make_runtime(database_path: string, root: string) {
	const workspace_git_registry = make_node_workspace_git_registry_layer([
		{ root, workspace_id },
	]) as unknown as Layer.Layer<WorkspaceGitRegistry, WorkspaceGitRegistrationError>;

	return make_backend_runtime({
		database_path,
		migrations_path,
		workspace_git_registry,
	});
}

const SeedThread = Effect.gen(function* () {
	const router = yield* ProtocolRouter;

	yield* router.Route({
		kind: "command",
		message_id: "create_git_protocol_thread",
		origin: "frontend",
		payload: { title: "Git protocol", type: "thread.create" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
		thread_id,
	});
});

function find_event(envelopes: ReadonlyArray<OutboundControlEnvelope>, type: string) {
	const event = envelopes.find(
		(envelope) => envelope.kind === "event" && envelope.payload.type === type,
	);

	if (event?.kind !== "event") {
		throw new Error(`Expected event ${type}`);
	}

	return event;
}

function find_receipt(envelopes: ReadonlyArray<OutboundControlEnvelope>) {
	const receipt = envelopes.find((envelope) => envelope.kind === "command.receipt");

	if (receipt?.kind !== "command.receipt") {
		throw new Error("Expected command receipt");
	}

	return receipt;
}

function receipt_journal_sequence(envelopes: ReadonlyArray<OutboundControlEnvelope>) {
	const receipt = find_receipt(envelopes);

	if (receipt.payload.status === "rejected" || receipt.payload.journal_sequence === undefined) {
		throw new Error("Expected a successful command receipt");
	}

	return receipt.payload.journal_sequence;
}

async function git(root: string, ...args: Array<string>) {
	const result = await exec_file("git", args, { cwd: root });

	return result.stdout.trim();
}

async function worktree_paths(root: string) {
	return (await git(root, "worktree", "list", "--porcelain"))
		.split(/\r?\n/)
		.filter((line) => line.startsWith("worktree "));
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("workspace Git protocol", () => {
	it("refreshes and replays a path-free session at its journal cursor", async () => {
		const { database_path, root } = await make_repository();
		const runtime = make_runtime(database_path, root);

		try {
			await runtime.runPromise(SeedThread);
			await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;
						const initial_handshake = yield* negotiate(connection);
						const initial_replay = initial_handshake.find(
							(envelope) => envelope.kind === "replay.complete",
						);

						if (initial_replay?.kind !== "replay.complete") {
							return yield* Effect.die("Initial replay did not complete");
						}

						yield* connection.Receive(session_query("session_before_refresh"));
						const absent = yield* take_outbound(connection, 1);

						yield* connection.Receive(session_refresh("session_refresh"));
						const refreshed = yield* take_outbound(connection, 2);
						const receipt = find_receipt(refreshed);
						const refresh_sequence = receipt_journal_sequence(refreshed);
						const event = find_event(refreshed, "workspace.git.session.updated");

						expect(absent).toMatchObject([
							{
								correlation_id: "session_before_refresh",
								kind: "workspace.git.session.query.result",
								payload: { journal_sequence: expect.any(Number) },
							},
						]);
						expect(absent[0]).not.toHaveProperty("payload.session");
						expect(receipt.payload).toMatchObject({ status: "accepted" });
						expect(event).toMatchObject({
							journal_sequence: refresh_sequence,
							payload: { session: { branch: "main", state: "ready", version: 1 } },
						});
						expect(JSON.stringify(event)).not.toContain(root);

						yield* connection.Receive(session_query("session_after_refresh"));
						const query = yield* take_outbound(connection, 1);
						expect(query).toMatchObject([
							{
								payload: {
									journal_sequence: expect.any(Number),
									session: {
										branch: "main",
										journal_sequence: refresh_sequence,
										version: 1,
									},
								},
							},
						]);
						expect(
							query[0]?.kind === "workspace.git.session.query.result"
								? query[0].payload.journal_sequence
								: 0,
						).toBeGreaterThanOrEqual(refresh_sequence);

						yield* connection.Close;
						const replay_connection = yield* open_connection;
						const replay = yield* negotiate(
							replay_connection,
							make_hello(
								"hello_replay",
								refresh_sequence - 1,
								initial_replay.payload.current_event_cursors,
							),
						);

						expect(replay).toEqual(
							expect.arrayContaining([
								expect.objectContaining({
									journal_sequence: refresh_sequence,
									kind: "event",
									payload: expect.objectContaining({
										type: "workspace.git.session.updated",
									}),
								}),
							]),
						);
					}),
				),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("returns stable errors and makes denial receipts idempotent", async () => {
		const { database_path, root } = await make_repository();
		const runtime = make_runtime(database_path, root);

		try {
			await runtime.runPromise(SeedThread);
			await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;
						yield* negotiate(connection);
						yield* connection.Receive(session_refresh("refresh_for_approval"));
						yield* take_outbound(connection, 2);

						yield* connection.Receive(checkout_request("checkout_request", 1));
						const requested = yield* take_outbound(connection, 2);
						const approval_event = find_event(
							requested,
							"workspace.git.checkout.approval.updated",
						);
						const approval_id =
							approval_event.payload.type ===
							"workspace.git.checkout.approval.updated"
								? approval_event.payload.approval.approval_id
								: "";

						yield* connection.Receive(approval_query("approval_query", approval_id));
						expect(yield* take_outbound(connection, 1)).toMatchObject([
							{ payload: { approval: { approval_id, state: "requested" } } },
						]);

						yield* connection.Receive(
							approval_response("deny_approval", approval_id, false),
						);
						const denied = yield* take_outbound(connection, 2);
						expect(denied).toMatchObject([
							{ kind: "command.receipt", payload: { status: "accepted" } },
							{ payload: { approval: { state: "denied" } } },
						]);

						yield* connection.Receive(
							approval_response("deny_approval", approval_id, false),
						);
						const duplicate_denial = yield* take_outbound(connection, 1);
						expect(duplicate_denial).toEqual([
							{
								...duplicate_denial[0],
								payload: expect.objectContaining({ status: "duplicate" }),
							},
						]);
						expect(receipt_journal_sequence(duplicate_denial)).toBe(
							receipt_journal_sequence(denied),
						);

						for (const [message_id, request] of [
							[
								"malformed_branch",
								checkout_request("malformed_branch", 1, "bad branch"),
							],
							["stale_request", checkout_request("stale_request", 99)],
							["missing_branch", checkout_request("missing_branch", 1, "missing")],
						] as const) {
							yield* connection.Receive({ ...request, message_id });
							const response = yield* take_outbound(connection, 1);

							expect(response).toMatchObject([
								{
									kind: "command.receipt",
									payload: {
										status: "rejected",
										error: { retryable: false },
									},
								},
							]);
						}
					}),
				),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("checks out one visible branch, preserves ordinary diff, and does not create Git artifacts", async () => {
		const { database_path, root } = await make_repository();
		const runtime = make_runtime(database_path, root);

		try {
			await runtime.runPromise(SeedThread);
			const before = {
				branches: await git(root, "for-each-ref", "--format=%(refname)", "refs/heads"),
				commits: await git(root, "rev-list", "--all", "--count"),
				worktrees: await worktree_paths(root),
			};

			await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;
						yield* negotiate(connection);
						yield* connection.Receive(session_refresh("checkout_refresh"));
						yield* take_outbound(connection, 2);
						yield* connection.Receive(checkout_request("checkout_request_success", 1));
						const requested = yield* take_outbound(connection, 2);
						const approval_event = find_event(
							requested,
							"workspace.git.checkout.approval.updated",
						);
						const approval_id =
							approval_event.payload.type ===
							"workspace.git.checkout.approval.updated"
								? approval_event.payload.approval.approval_id
								: "";

						yield* connection.Receive(
							approval_response("approve_checkout", approval_id, true),
						);
						const applied = yield* take_until_outbound(
							connection,
							(envelope) =>
								envelope.kind === "event" &&
								envelope.payload.type ===
									"workspace.git.checkout.approval.updated" &&
								envelope.payload.approval.state === "applied",
						);
						const lifecycle = applied
							.filter(
								(envelope) =>
									envelope.kind === "event" &&
									envelope.payload.type ===
										"workspace.git.checkout.approval.updated",
							)
							.map((envelope) => {
								if (
									envelope.kind !== "event" ||
									envelope.payload.type !==
										"workspace.git.checkout.approval.updated"
								) {
									return "";
								}

								return envelope.payload.approval.state;
							});

						expect(lifecycle).toEqual(["approved", "executing", "applied"]);
						expect(
							applied.find((envelope) => envelope.kind === "command.receipt"),
						).toMatchObject({
							kind: "command.receipt",
							payload: { status: "accepted" },
						});
						expect(
							applied.find(
								(envelope) =>
									envelope.kind === "event" &&
									envelope.payload.type === "workspace.git.session.updated",
							),
						).toBeDefined();
					}),
				),
			);

			await writeFile(join(root, "accepted.txt"), "accepted after checkout\n");
			expect(await git(root, "branch", "--show-current")).toBe("feature/git-session");
			expect(await git(root, "diff", "--name-only")).toBe("accepted.txt");
			expect(await readFile(join(root, "accepted.txt"), "utf8")).toBe(
				"accepted after checkout\n",
			);
			expect(await git(root, "for-each-ref", "--format=%(refname)", "refs/heads")).toBe(
				before.branches,
			);
			expect(await worktree_paths(root)).toEqual(before.worktrees);
			expect(await git(root, "rev-list", "--all", "--count")).toBe(before.commits);
		} finally {
			await runtime.dispose();
		}
	});

	it("reports an unregistered non-Git workspace as unavailable", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-non-git-"));
		const root = join(directory, "plain");
		const database_path = join(directory, "artisan.db");

		temporary_directories.push(directory);
		await mkdir(root, { recursive: true });
		const runtime = make_runtime(database_path, root);

		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;
						yield* negotiate(connection);
						yield* connection.Receive(
							session_query("non_git_query", "non_git_workspace"),
						);

						return yield* take_outbound(connection, 1);
					}),
				),
			);

			expect(result).toMatchObject([
				{
					kind: "workspace.git.session.query.result",
					payload: { journal_sequence: expect.any(Number) },
				},
			]);
			expect(result[0]).not.toHaveProperty("payload.session");
		} finally {
			await runtime.dispose();
		}
	});
});

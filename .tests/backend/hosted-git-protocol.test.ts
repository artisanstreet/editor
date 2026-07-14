import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_backend_runtime,
	make_node_workspace_git_registry_layer,
	ProtocolRouter,
	ProtocolServer,
	type ProtocolConnection,
} from "@artisan/backend";
import type {
	HelloEnvelope,
	HostedGitCheckFailureDetailQueryEnvelope,
	HostedGitSnapshotQueryEnvelope,
	HostedGitSnapshotRefreshEnvelope,
	OutboundControlEnvelope,
} from "@artisan/protocol";

import {
	GitProviderError,
	type GitProvider,
} from "../../modules/backend/src/git-provider/git-provider";
import {
	GitProviderRegistry,
	GitProviderRegistryError,
	make_git_provider_registry_layer,
} from "../../modules/backend/src/git-provider/git-provider-registry";
import {
	WorkspaceGitRegistrationError,
	WorkspaceGitRegistry,
} from "../../modules/backend/src/git/workspace-git-registry";
import { ProjectRepository } from "../../modules/backend/src/projects/project-repository";

const exec_file = promisify(execFile);
const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const protocol_time = "2026-07-14T16:00:00.000Z";
const thread_id = "thread_hosted_git_protocol";
const project_native_id = "repository_1";

interface ProviderState {
	calls: number;
	detail_calls: number;
	detail_failure?: GitProviderError;
	failure?: GitProviderError;
}

const pull_request_origin = {
	native_id: "pull-request-42",
	provider_id: "github",
	resource_kind: "pull_request" as const,
};
const check_origin = {
	native_id: "check-run-7",
	provider_id: "github",
	resource_kind: "check_run" as const,
};
const workflow_origin = {
	native_id: "workflow-run-9",
	provider_id: "github",
	resource_kind: "workflow_run" as const,
};

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

function make_hello(message_id: string): HelloEnvelope {
	return {
		kind: "hello",
		message_id,
		origin: "frontend",
		payload: {
			event_cursors: [],
			last_journal_sequence: 0,
			supported_protocol_versions: [1],
		},
		schema_version: 1,
		sent_at: protocol_time,
	};
}

const Negotiate = (connection: ProtocolConnection, message_id: string) =>
	Effect.gen(function* () {
		yield* connection.Receive(make_hello(message_id));

		return yield* take_until_outbound(
			connection,
			(envelope) => envelope.kind === "replay.complete",
		);
	});

function snapshot_query(message_id: string, workspace_id: string): HostedGitSnapshotQueryEnvelope {
	return {
		kind: "hosted.git.snapshot.query",
		message_id,
		origin: "frontend",
		payload: { workspace_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
	};
}

function snapshot_refresh(
	message_id: string,
	workspace_id: string,
): HostedGitSnapshotRefreshEnvelope {
	return {
		kind: "hosted.git.snapshot.refresh",
		message_id,
		origin: "frontend",
		payload: { workspace_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
		thread_id,
	};
}

function check_failure_detail_query(
	message_id: string,
	workspace_id: string,
	expected_head_commit: string,
	snapshot_version: number,
): HostedGitCheckFailureDetailQueryEnvelope {
	return {
		kind: "hosted.git.check_failure_detail.query",
		message_id,
		origin: "frontend",
		payload: {
			check_origin,
			expected_head_commit,
			snapshot_version,
			workspace_id,
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
	};
}

function derive_workspace_id() {
	const parts = ["github", "github.com", project_native_id].map((part) =>
		Buffer.from(part, "utf8"),
	);
	const framed = Buffer.alloc(parts.reduce((size, part) => size + 4 + part.length, 0));
	let offset = 0;

	for (const part of parts) {
		framed.writeUInt32BE(part.length, offset);
		offset += 4;
		part.copy(framed, offset);
		offset += part.length;
	}

	return `workspace_${createHash("sha256").update(framed).digest("hex")}`;
}

async function make_repository() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-hosted-git-protocol-"));
	const root = join(directory, "repository");

	temporary_directories.push(directory);
	await mkdir(root, { recursive: true });
	await exec_file("git", ["init", "-b", "main"], { cwd: root });
	await exec_file("git", ["config", "user.email", "protocol@example.test"], {
		cwd: root,
	});
	await exec_file("git", ["config", "user.name", "Protocol Test"], { cwd: root });
	await writeFile(join(root, "accepted.txt"), "main\n");
	await exec_file("git", ["add", "accepted.txt"], { cwd: root });
	await exec_file("git", ["commit", "-m", "initial"], { cwd: root });

	return {
		database_path: join(directory, "artisan.db"),
		root: await realpath(root),
		workspace_id: derive_workspace_id(),
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
		ReadCheckFailureDetail: (input) => {
			state.detail_calls += 1;

			if (state.detail_failure !== undefined) {
				return Effect.fail(state.detail_failure);
			}

			return Effect.succeed({
				attempt: 2,
				check_origin: input.check_origin,
				head_commit: input.expected_head,
				log: {
					_tag: "available",
					observed_bytes: 36,
					truncated: false,
					untrusted_excerpt: "transient protocol failure excerpt",
				},
				name: "test",
				output: {
					summary: {
						_tag: "available",
						truncated: false,
						untrusted_text: "One test failed",
					},
					text: { _tag: "unavailable" },
				},
				workflow_origin,
			});
		},
		ReadPullRequest: (input) => {
			state.calls += 1;

			if (state.failure !== undefined) {
				return Effect.fail(state.failure);
			}

			return Effect.succeed({
				association: {
					_tag: "matched",
					freshness: "current",
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
								state: "failed",
								workflow_name: "CI",
								workflow_origin,
							},
						],
						checks_total: 1,
						checks_truncated: false,
						draft: false,
						head_branch: input.selected_branch,
						head_commit: input.expected_head,
						mergeability: "mergeable",
						number: 42,
						origin: pull_request_origin,
						requested_reviewers: [],
						requested_reviewers_truncated: false,
						review_decision: "none",
						review_threads: [],
						review_threads_total: 0,
						review_threads_truncated: false,
						reviews: [],
						reviews_total: 0,
						reviews_truncated: false,
						state: "open",
						title: "Read hosted check detail",
						web_url: "https://github.com/artisan/editor/pull/42",
					},
				},
				branch: input.selected_branch,
				expected_head_commit: input.expected_head,
				repository: input.repository,
			});
		},
	};
}

function make_runtime(
	database_path: string,
	root: string,
	workspace_id: string,
	provider_state: ProviderState,
) {
	const workspace_git_registry = make_node_workspace_git_registry_layer([
		{ root, workspace_id },
	]).pipe(Layer.provide(NodeFileSystem.layer)) as unknown as Layer.Layer<
		WorkspaceGitRegistry,
		WorkspaceGitRegistrationError
	>;
	const git_provider_registry = make_git_provider_registry_layer([
		{
			hosts: ["github.com"],
			provider: make_provider(provider_state),
		},
	]) as Layer.Layer<GitProviderRegistry, GitProviderRegistryError>;

	return make_backend_runtime({
		database_path,
		git_provider_registry,
		migrations_path,
		workspace_git_registry,
	});
}

const SeedProjectThread = (root: string) =>
	Effect.gen(function* () {
		const projects = yield* ProjectRepository;
		const router = yield* ProtocolRouter;

		yield* router.Route({
			kind: "command",
			message_id: "create_hosted_git_protocol_thread",
			origin: "frontend",
			payload: { title: "Hosted Git protocol", type: "thread.create" },
			protocol_version: 1,
			schema_version: 1,
			sent_at: protocol_time,
			thread_id,
		});
		const registration = yield* projects.RegisterHosted({
			canonical_root: root,
			display_name: "Artisan Editor",
			hosted_origin: {
				canonical_host: "github.com",
				clone_url: "https://github.com/artisan/editor.git",
				fetch_url: "https://github.com/artisan/editor.git",
				name: "editor",
				native_id: project_native_id,
				owner: "artisan",
				provider_id: "github",
				push_url: "https://github.com/artisan/editor.git",
				remote_name: "origin",
				selected_account_login: "alice",
				web_url: "https://github.com/artisan/editor",
			},
		});

		yield* router.Route({
			kind: "command",
			message_id: "assign_hosted_git_protocol_project",
			origin: "frontend",
			payload: {
				project: registration.project.project,
				type: "thread.project.assign",
			},
			protocol_version: 1,
			schema_version: 1,
			sent_at: protocol_time,
			thread_id,
		});

		return registration.project;
	});

function find_receipt(envelopes: ReadonlyArray<OutboundControlEnvelope>) {
	const receipt = envelopes.find((envelope) => envelope.kind === "command.receipt");

	if (receipt?.kind !== "command.receipt") {
		throw new Error("Expected command receipt");
	}

	return receipt;
}

function find_query_result(envelopes: ReadonlyArray<OutboundControlEnvelope>) {
	const result = envelopes.find(
		(envelope) => envelope.kind === "hosted.git.snapshot.query.result",
	);

	if (result?.kind !== "hosted.git.snapshot.query.result") {
		throw new Error("Expected hosted Git snapshot query result");
	}

	return result;
}

function find_check_failure_detail_result(envelopes: ReadonlyArray<OutboundControlEnvelope>) {
	const result = envelopes.find(
		(envelope) => envelope.kind === "hosted.git.check_failure_detail.query.result",
	);

	if (result?.kind !== "hosted.git.check_failure_detail.query.result") {
		throw new Error("Expected hosted Git check failure detail result");
	}

	return result;
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("hosted Git protocol", () => {
	it("replays a refresh across restart and marks the cached projection stale after a local commit", async () => {
		const fixture = await make_repository();
		const provider_state: ProviderState = { calls: 0, detail_calls: 0 };
		const first_runtime = make_runtime(
			fixture.database_path,
			fixture.root,
			fixture.workspace_id,
			provider_state,
		);

		await first_runtime.runPromise(SeedProjectThread(fixture.root));
		await first_runtime.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const connection = yield* open_connection;

					yield* Negotiate(connection, "hello_first");
					yield* connection.Receive(snapshot_query("query_before", fixture.workspace_id));
					const before = yield* take_outbound(connection, 1);

					yield* connection.Receive(
						snapshot_refresh("refresh_hosted_state", fixture.workspace_id),
					);
					const refreshed = yield* take_outbound(connection, 2);
					const snapshot_event = refreshed.find(
						(envelope) =>
							envelope.kind === "event" &&
							envelope.payload.type === "hosted.git.snapshot.updated",
					);

					expect(find_query_result(before).payload.snapshot).toBeUndefined();
					expect(find_receipt(refreshed).payload).toMatchObject({ status: "accepted" });
					expect(snapshot_event).toMatchObject({
						kind: "event",
						payload: {
							snapshot: { workspace_freshness: "unverified" },
							type: "hosted.git.snapshot.updated",
						},
					});

					if (
						snapshot_event?.kind !== "event" ||
						snapshot_event.payload.type !== "hosted.git.snapshot.updated"
					) {
						throw new Error("Expected hosted Git snapshot event");
					}

					yield* connection.Receive(
						check_failure_detail_query(
							"detail_before_restart",
							fixture.workspace_id,
							snapshot_event.payload.snapshot.lookup.expected_head_commit,
							snapshot_event.payload.snapshot.version,
						),
					);
					const detail = yield* take_outbound(connection, 1);

					expect(find_check_failure_detail_result(detail)).toMatchObject({
						correlation_id: "detail_before_restart",
						payload: {
							detail: {
								check_origin,
								log: { untrusted_excerpt: "transient protocol failure excerpt" },
							},
							snapshot_version: 1,
						},
					});
				}),
			),
		);

		await first_runtime.dispose();

		const second_runtime = make_runtime(
			fixture.database_path,
			fixture.root,
			fixture.workspace_id,
			provider_state,
		);

		try {
			await second_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* Negotiate(connection, "hello_second");
						yield* connection.Receive(
							snapshot_refresh("refresh_hosted_state", fixture.workspace_id),
						);
						const replayed = yield* take_outbound(connection, 1);

						expect(find_receipt(replayed).payload).toMatchObject({
							status: "duplicate",
						});

						yield* connection.Receive(
							snapshot_query("query_current", fixture.workspace_id),
						);
						const current = yield* take_outbound(connection, 1);

						expect(
							find_query_result(current).payload.snapshot?.workspace_freshness,
						).toBe("current");

						const current_snapshot = find_query_result(current).payload.snapshot;

						if (current_snapshot === undefined) {
							throw new Error("Expected current hosted Git snapshot");
						}

						yield* connection.Receive(
							check_failure_detail_query(
								"detail_after_restart",
								fixture.workspace_id,
								current_snapshot.lookup.expected_head_commit,
								current_snapshot.version,
							),
						);
						const detail = yield* take_outbound(connection, 1);

						expect(find_check_failure_detail_result(detail)).toMatchObject({
							correlation_id: "detail_after_restart",
						});

						provider_state.detail_failure = new GitProviderError({
							host: "github.com",
							operation: "read_check_failure_detail",
							provider_id: "github",
							reason: "rate_limited",
							retryable: true,
						});
						yield* connection.Receive(
							check_failure_detail_query(
								"detail_rate_limited",
								fixture.workspace_id,
								current_snapshot.lookup.expected_head_commit,
								current_snapshot.version,
							),
						);
						const rejected = yield* take_outbound(connection, 1);

						expect(rejected).toContainEqual(
							expect.objectContaining({
								correlation_id: "detail_rate_limited",
								kind: "protocol.error",
								payload: expect.objectContaining({
									code: "hosted.git.check_failure_detail_rate_limited",
									retryable: true,
								}),
							}),
						);
						delete provider_state.detail_failure;
					}),
				),
			);

			await writeFile(join(fixture.root, "accepted.txt"), "changed\n");
			await exec_file("git", ["add", "accepted.txt"], { cwd: fixture.root });
			await exec_file("git", ["commit", "-m", "change head"], { cwd: fixture.root });

			await second_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* Negotiate(connection, "hello_stale");
						yield* connection.Receive(
							snapshot_query("query_stale", fixture.workspace_id),
						);
						const stale = yield* take_outbound(connection, 1);

						expect(find_query_result(stale).payload.snapshot?.workspace_freshness).toBe(
							"stale_local_git",
						);
					}),
				),
			);

			provider_state.failure = new GitProviderError({
				host: "github.com",
				operation: "read_pull_request",
				provider_id: "github",
				reason: "auth_required",
				retryable: false,
			});

			await second_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* Negotiate(connection, "hello_auth_required");
						yield* connection.Receive(
							snapshot_refresh("refresh_auth_required", fixture.workspace_id),
						);
						const rejected = yield* take_outbound(connection, 1);

						expect(find_receipt(rejected).payload).toMatchObject({
							error: {
								code: "hosted.git.snapshot_authentication_required",
								retryable: false,
							},
							status: "rejected",
						});
					}),
				),
			);

			provider_state.failure = new GitProviderError({
				host: "github.com",
				operation: "read_pull_request",
				provider_id: "github",
				reason: "rate_limited",
				retryable: true,
			});

			await second_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* Negotiate(connection, "hello_rate_limited");
						yield* connection.Receive(
							snapshot_refresh("refresh_rate_limited", fixture.workspace_id),
						);
						const rejected = yield* take_outbound(connection, 1);

						expect(find_receipt(rejected).payload).toMatchObject({
							error: {
								code: "hosted.git.snapshot_rate_limited",
								retryable: true,
							},
							status: "rejected",
						});
					}),
				),
			);

			expect(provider_state.calls).toBe(3);
			expect(provider_state.detail_calls).toBe(3);
		} finally {
			await second_runtime.dispose();
		}
	});
});

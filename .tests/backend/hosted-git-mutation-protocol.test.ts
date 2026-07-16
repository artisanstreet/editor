import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime, ProtocolServer, type ProtocolConnection } from "@artisan/backend";
import type {
	HelloEnvelope,
	HostedGitMutationApprovalQueryEnvelope,
	HostedGitMutationApprovalRespondEnvelope,
	HostedGitMutationApprovalUpdatedEvent,
	HostedGitMutationRequestEnvelope,
	OutboundControlEnvelope,
} from "@artisan/protocol";

import type { GitProvider as GitProviderService } from "../../modules/backend/src/git-provider/git-provider";
import {
	GitProviderRegistry,
	GitProviderRegistryError,
	make_git_provider_registry_layer,
} from "../../modules/backend/src/git-provider/git-provider-registry";
import { Database } from "../../modules/backend/src/persistence/database";
import {
	HostedGitSnapshots,
	ProjectHostedOrigins,
	Projects,
	Threads,
	WorkspaceGitSessions,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const protocol_time = "2026-07-16T14:00:00.000Z";
const thread_id = "thread_hosted_git_mutation_protocol";
const head = "a".repeat(40);
const repository = { host: "github.com", name: "editor", owner: "artisan", provider_id: "github" };
const selection = { account_login: "alice", host: "github.com", provider_id: "github" };
const pull_request_origin = {
	native_id: "PR_42",
	provider_id: "github",
	resource_kind: "pull_request" as const,
};
const review_thread_origin = {
	native_id: "RT_7",
	provider_id: "github",
	resource_kind: "review_thread" as const,
};

interface ProviderState {
	calls: number;
}

const OpenConnection = Effect.gen(function* () {
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

const Negotiate = (connection: ProtocolConnection) =>
	Effect.gen(function* () {
		yield* connection.Receive(make_hello("hello_hosted_git_mutation"));

		yield* take_until_outbound(connection, (envelope) => envelope.kind === "replay.complete");
	});

function mutation_request(
	message_id: string,
	body = "private review reply",
): HostedGitMutationRequestEnvelope {
	return {
		kind: "hosted.git.mutation.request",
		message_id,
		origin: "frontend",
		payload: {
			mutation: {
				body,
				expected_head_commit: head,
				operation: "reply_review_thread",
				pull_request_number: 42,
				pull_request_origin,
				repository,
				selected_branch: "feature",
				snapshot_version: 1,
				thread_origin: review_thread_origin,
				workspace_id: "workspace_1",
			},
			selection,
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
		thread_id,
	};
}

function approval_query(
	message_id: string,
	approval_id: string,
): HostedGitMutationApprovalQueryEnvelope {
	return {
		kind: "hosted.git.mutation.approval.query",
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
): HostedGitMutationApprovalRespondEnvelope {
	return {
		kind: "hosted.git.mutation.approval.respond",
		message_id,
		origin: "frontend",
		payload: { approval_id, approved: true },
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
		thread_id,
	};
}

function make_provider(state: ProviderState): typeof GitProviderService.Service {
	return {
		Clone: () => Effect.die("unused"),
		Descriptor: {
			capabilities: [{ _tag: "available", capability: "write_provider_mutations" }],
			display_name: "GitHub",
			provider_id: "github",
		},
		DiscoverRepositories: () => Effect.die("unused"),
		ExecuteMutation: (input) => {
			state.calls += 1;

			return Effect.succeed({
				operation: "reply_review_thread" as const,
				origin: {
					native_id: `RC_${input.client_mutation_id}`,
					provider_id: "github",
					resource_kind: "review_comment" as const,
				},
				status: "applied" as const,
				thread_origin: review_thread_origin,
			});
		},
		Inspect: Effect.die("unused"),
		PrepareClone: () => Effect.die("unused"),
	};
}

function make_runtime(database_path: string, provider_state: ProviderState) {
	const git_provider_registry = make_git_provider_registry_layer([
		{ hosts: ["github.com"], provider: make_provider(provider_state) },
	]) as Layer.Layer<GitProviderRegistry, GitProviderRegistryError>;

	return make_backend_runtime({ database_path, git_provider_registry, migrations_path });
}

const Seed = Effect.gen(function* () {
	const database = yield* Database;
	const lookup = {
		association: {
			_tag: "matched" as const,
			freshness: "current" as const,
			pull_request: {
				base_branch: "main",
				base_commit: "b".repeat(40),
				checks: [],
				checks_total: 0,
				checks_truncated: false,
				draft: false,
				head_branch: "feature",
				head_commit: head,
				mergeability: "mergeable" as const,
				number: 42,
				origin: pull_request_origin,
				requested_reviewers: [],
				requested_reviewers_truncated: false,
				review_decision: "none" as const,
				review_threads: [
					{
						comment_count: 1,
						origin: review_thread_origin,
						outdated: false,
						path: "src/main.ts",
						resolved: false,
						subject: "line",
					},
				],
				review_threads_total: 1,
				review_threads_truncated: false,
				reviews: [],
				reviews_total: 0,
				reviews_truncated: false,
				state: "open" as const,
				title: "Mutation target",
				web_url: "https://github.com/artisan/editor/pull/42",
			},
		},
		branch: "feature",
		expected_head_commit: head,
		repository,
	};

	yield* database.client.insert(Projects).values({
		canonical_root: "C:/project",
		display_name: "Artisan",
		project_id: "project_1",
		registered_at: protocol_time,
		updated_at: protocol_time,
		workspace_id: "workspace_1",
	});
	yield* database.client.insert(ProjectHostedOrigins).values({
		canonical_host: "github.com",
		clone_url: "https://github.com/artisan/editor.git",
		fetch_url: "https://github.com/artisan/editor.git",
		name: "editor",
		native_id: "repository_1",
		owner: "artisan",
		project_id: "project_1",
		provider_id: "github",
		push_url: "https://github.com/artisan/editor.git",
		remote_name: "origin",
		selected_account_login: "alice",
		web_url: "https://github.com/artisan/editor",
	});
	yield* database.client.insert(WorkspaceGitSessions).values({
		additions: 0,
		blockers_json: "[]",
		branch: "feature",
		deletions: 0,
		files: 0,
		has_diff: false,
		head,
		journal_sequence: 1,
		observed_at: protocol_time,
		repository_root: "C:/project",
		selected_worktree_path: "C:/project",
		state: "ready",
		updated_at: protocol_time,
		version: 1,
		workspace_id: "workspace_1",
	});
	yield* database.client.insert(Threads).values({
		created_at: protocol_time,
		primary_project_id: "project_1",
		primary_project_json: JSON.stringify({
			display_name: "Artisan",
			project_id: "project_1",
			root_path: "C:/project",
		}),
		thread_id,
		title: "Hosted mutation",
		title_source: "initial",
		updated_at: protocol_time,
	});
	yield* database.client.insert(HostedGitSnapshots).values({
		journal_sequence: 1,
		lookup_json: JSON.stringify(lookup),
		observed_at: protocol_time,
		project_id: "project_1",
		version: 1,
	});
});

function find_event(
	envelopes: ReadonlyArray<OutboundControlEnvelope>,
	state?: string,
): Extract<OutboundControlEnvelope, { readonly kind: "event" }> & {
	readonly payload: HostedGitMutationApprovalUpdatedEvent;
} {
	const event = envelopes.find(
		(envelope) =>
			envelope.kind === "event" &&
			envelope.payload.type === "hosted.git.mutation.approval.updated" &&
			(state === undefined || envelope.payload.approval.state === state),
	);

	if (event?.kind !== "event" || event.payload.type !== "hosted.git.mutation.approval.updated") {
		throw new Error("Expected hosted Git mutation approval event");
	}

	return event as Extract<OutboundControlEnvelope, { readonly kind: "event" }> & {
		readonly payload: HostedGitMutationApprovalUpdatedEvent;
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("hosted Git mutation protocol", () => {
	it("prepares, approves, applies, and queries one hosted mutation through the production runtime", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-hosted-git-mutation-protocol-"));
		const provider_state = { calls: 0 };
		const runtime = make_runtime(join(directory, "artisan.db"), provider_state);

		temporary_directories.push(directory);

		try {
			await runtime.runPromise(Seed);
			await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* OpenConnection;

						yield* Negotiate(connection);
						yield* connection.Receive(mutation_request("request_mutation"));
						const requested = yield* take_outbound(connection, 2);
						const approval_id = find_event(requested, "requested").payload.approval
							.approval_id;

						expect(requested).toContainEqual(
							expect.objectContaining({
								kind: "command.receipt",
								payload: expect.objectContaining({ status: "accepted" }),
							}),
						);
						yield* connection.Receive(approval_query("query_requested", approval_id));
						const queried_requested = yield* take_outbound(connection, 1);

						expect(queried_requested).toMatchObject([
							{
								correlation_id: "query_requested",
								kind: "hosted.git.mutation.approval.query.result",
								payload: { approval: { approval_id, state: "requested" } },
							},
						]);
						yield* connection.Receive(
							approval_response("approve_mutation", approval_id),
						);
						const applied = yield* take_until_outbound(
							connection,
							(envelope) =>
								envelope.kind === "event" &&
								envelope.payload.type === "hosted.git.mutation.approval.updated" &&
								envelope.payload.approval.state === "applied",
						);

						const applied_event = find_event(applied, "applied");

						if (applied_event.payload.approval.state !== "applied") {
							throw new Error("Expected applied hosted Git mutation approval");
						}

						expect(applied_event.payload.approval.result).toMatchObject({
							status: "applied",
						});
						expect(applied).toContainEqual(
							expect.objectContaining({
								kind: "command.receipt",
								payload: expect.objectContaining({ status: "accepted" }),
							}),
						);
						yield* connection.Receive(approval_query("query_applied", approval_id));
						const queried_applied = yield* take_outbound(connection, 1);

						expect(queried_applied).toMatchObject([
							{
								correlation_id: "query_applied",
								payload: { approval: { approval_id, state: "applied" } },
							},
						]);
						yield* connection.Receive(
							mutation_request("request_mutation", "changed private review reply"),
						);
						const changed_intent = yield* take_outbound(connection, 1);

						expect(changed_intent).toMatchObject([
							{
								kind: "command.receipt",
								payload: {
									error: {
										code: "hosted.git.mutation_request_conflict",
										retryable: false,
									},
									status: "rejected",
								},
							},
						]);
					}),
				),
			);

			expect(provider_state.calls).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});
});

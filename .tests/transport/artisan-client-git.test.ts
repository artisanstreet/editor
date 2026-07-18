import { Deferred, Effect, Queue, Stream } from "effect";
import { describe, expect, it } from "vitest";

import { ProtocolServer, type ProtocolConnection } from "@artisan/backend";
import {
	DecodeInboundControlEnvelope,
	type GitDiffQueryEnvelope,
	type GitIndexStageRequestEnvelope,
	type GitIndexUnstageRequestEnvelope,
	type GitMutationResolveEnvelope,
	type GitWorkspaceQueryEnvelope,
	type HelloEnvelope,
	type InboundControlEnvelope,
	type OutboundControlEnvelope,
} from "@artisan/protocol";
import { ArtisanClientError } from "@artisan/transport";

import {
	make_transport_test_harness_with_protocol_server,
	wait_for,
} from "./message-channel-harness";

const timestamp = "2026-07-18T08:00:00.000Z";
const snapshot_id = "b".repeat(64);

type GitMutationEnvelope =
	| GitIndexStageRequestEnvelope
	| GitIndexUnstageRequestEnvelope
	| GitMutationResolveEnvelope;

interface GitProtocolSnapshot {
	readonly diff_queries: ReadonlyArray<GitDiffQueryEnvelope>;
	readonly mutation_attempts: ReadonlyArray<GitMutationEnvelope>;
	readonly workspace_queries: ReadonlyArray<GitWorkspaceQueryEnvelope>;
}

function make_git_protocol_server() {
	const diff_queries: Array<GitDiffQueryEnvelope> = [];
	const mutation_attempts: Array<GitMutationEnvelope> = [];
	const workspace_queries: Array<GitWorkspaceQueryEnvelope> = [];
	const accepted_mutations = new Map<
		string,
		{ readonly fingerprint: string; readonly journal_sequence: number }
	>();
	let next_backend_id = 0;
	let next_journal_sequence = 10;

	const backend_trace = () => ({
		message_id: `git_backend_${++next_backend_id}`,
		origin: "backend" as const,
		protocol_version: 1 as const,
		schema_version: 1 as const,
		sent_at: timestamp,
	});
	const open = Effect.gen(function* () {
		const outbound = yield* Effect.acquireRelease(
			Queue.unbounded<OutboundControlEnvelope>(),
			Queue.shutdown,
		);
		const closed = yield* Deferred.make<void>();
		let negotiated = false;
		let locally_closed = false;

		const enqueue = (envelope: OutboundControlEnvelope) =>
			Queue.offer(outbound, envelope).pipe(Effect.asVoid);
		const close = Effect.gen(function* () {
			if (locally_closed) {
				return;
			}

			locally_closed = true;
			yield* Queue.shutdown(outbound);
			yield* Deferred.succeed(closed, undefined);
		});

		yield* Effect.addFinalizer(() => close);

		const welcome = (hello: HelloEnvelope) =>
			enqueue({
				...backend_trace(),
				correlation_id: hello.message_id,
				kind: "welcome",
				payload: {
					connection_id: `git_protocol_${next_backend_id}`,
					current_event_cursors: [],
					heartbeat_interval_ms: 15_000,
					heartbeat_timeout_ms: 45_000,
					journal_sequence: next_journal_sequence,
					stream_ticket: `git_stream_${next_backend_id}`,
				},
			});
		const handle_workspace_query = (query: GitWorkspaceQueryEnvelope) => {
			workspace_queries.push(query);

			return enqueue({
				...backend_trace(),
				correlation_id: query.message_id,
				kind: "git.workspace.query.result",
				payload: {
					journal_sequence: 5,
					pending_mutations: [],
					workspace: {
						journal_sequence: 5,
						observed_at: timestamp,
						repository_state: "not_repository",
						snapshot_id,
						version: 2,
						workspace_id: query.payload.workspace_id,
					},
				},
			});
		};
		const handle_diff_query = (query: GitDiffQueryEnvelope) => {
			diff_queries.push(query);

			if (query.payload.workspace_id === "workspace_rejected") {
				return enqueue({
					...backend_trace(),
					correlation_id: query.message_id,
					kind: "protocol.error",
					payload: {
						code: "git.changed",
						message: "The Git snapshot changed.",
						retryable: false,
					},
				});
			}

			const patch = "diff";

			return enqueue({
				...backend_trace(),
				correlation_id: query.message_id,
				kind: "git.diff.query.result",
				payload: {
					byte_count: patch.length,
					format: "unified",
					format_version: 1,
					patch,
					scope: query.payload.scope,
					snapshot_id: query.payload.expected_snapshot_id,
					truncated: false,
					workspace_id: query.payload.workspace_id,
					workspace_version: query.payload.expected_workspace_version,
				},
			});
		};
		const handle_mutation = (mutation: GitMutationEnvelope) => {
			mutation_attempts.push(mutation);

			if (
				mutation.kind !== "git.mutation.resolve" &&
				mutation.payload.paths.includes("reject.ts")
			) {
				return enqueue({
					...backend_trace(),
					causation_id: mutation.message_id,
					correlation_id: mutation.message_id,
					kind: "command.receipt",
					payload: {
						error: {
							code: "git.path_rejected",
							message: "The Git path was rejected.",
							retryable: false,
						},
						status: "rejected",
					},
					thread_id: mutation.thread_id,
				});
			}

			const fingerprint = JSON.stringify(mutation);
			const prior = accepted_mutations.get(mutation.message_id);

			if (prior && prior.fingerprint !== fingerprint) {
				return enqueue({
					...backend_trace(),
					causation_id: mutation.message_id,
					correlation_id: mutation.message_id,
					kind: "command.receipt",
					payload: {
						error: {
							code: "command.id_conflict",
							message: "The Git command id changed intent.",
							retryable: false,
						},
						status: "rejected",
					},
					thread_id: mutation.thread_id,
				});
			}

			if (prior) {
				return enqueue({
					...backend_trace(),
					causation_id: mutation.message_id,
					correlation_id: mutation.message_id,
					kind: "command.receipt",
					payload: {
						journal_sequence: prior.journal_sequence,
						status: "duplicate",
					},
					thread_id: mutation.thread_id,
				});
			}

			const journal_sequence = ++next_journal_sequence;

			accepted_mutations.set(mutation.message_id, { fingerprint, journal_sequence });

			return enqueue({
				...backend_trace(),
				causation_id: mutation.message_id,
				correlation_id: mutation.message_id,
				kind: "command.receipt",
				payload: { journal_sequence, status: "accepted" },
				thread_id: mutation.thread_id,
			});
		};
		const handle = (input: InboundControlEnvelope) => {
			if (input.kind === "hello") {
				negotiated = true;

				return welcome(input);
			}

			if (!negotiated) {
				return Effect.void;
			}

			switch (input.kind) {
				case "git.workspace.query":
					return handle_workspace_query(input);
				case "git.diff.query":
					return handle_diff_query(input);
				case "git.index.stage.request":
				case "git.index.unstage.request":
				case "git.mutation.resolve":
					return handle_mutation(input);
				default:
					return Effect.void;
			}
		};
		const receive = (input: unknown) =>
			DecodeInboundControlEnvelope(input).pipe(
				Effect.flatMap(handle),
				Effect.catch(() => Effect.void),
			);
		const connection: ProtocolConnection = {
			Close: close,
			Closed: Deferred.await(closed),
			Outbound: Stream.fromQueue(outbound),
			Receive: receive,
		};

		return connection;
	});
	const server: typeof ProtocolServer.Service = { Open: open };
	const snapshot = (): GitProtocolSnapshot => ({
		diff_queries: [...diff_queries],
		mutation_attempts: [...mutation_attempts],
		workspace_queries: [...workspace_queries],
	});

	return { server, snapshot };
}

describe("ArtisanClient Git surface", () => {
	it("returns typed Git workspace and diff results from exact query envelopes", async () => {
		const protocol = make_git_protocol_server();
		const harness = await make_transport_test_harness_with_protocol_server(protocol.server);

		try {
			const workspace = await Effect.runPromise(
				harness.client.GetGitWorkspace({
					thread_id: "thread_1",
					workspace_id: "workspace_1",
				}),
			);
			const diff = await Effect.runPromise(
				harness.client.GetGitDiff({
					expected_snapshot_id: snapshot_id,
					expected_workspace_version: 2,
					max_bytes: 4_096,
					scope: "aggregate",
					workspace_id: "workspace_1",
				}),
			);

			expect(workspace).toMatchObject({
				journal_sequence: 5,
				pending_mutations: [],
				workspace: {
					repository_state: "not_repository",
					snapshot_id,
					workspace_id: "workspace_1",
				},
			});
			expect(diff).toEqual({
				byte_count: 4,
				format: "unified",
				format_version: 1,
				patch: "diff",
				scope: "aggregate",
				snapshot_id,
				truncated: false,
				workspace_id: "workspace_1",
				workspace_version: 2,
			});
			expect(protocol.snapshot().workspace_queries[0]).toMatchObject({
				kind: "git.workspace.query",
				payload: { thread_id: "thread_1", workspace_id: "workspace_1" },
			});
			expect(protocol.snapshot().diff_queries[0]).toMatchObject({
				kind: "git.diff.query",
				payload: {
					expected_snapshot_id: snapshot_id,
					expected_workspace_version: 2,
					max_bytes: 4_096,
					scope: "aggregate",
					workspace_id: "workspace_1",
				},
			});
		} finally {
			await harness.dispose();
		}
	});

	it("builds exact stage, unstage, and resolution envelopes with stable ids", async () => {
		const protocol = make_git_protocol_server();
		const harness = await make_transport_test_harness_with_protocol_server(protocol.server);

		try {
			const generated = await Effect.runPromise(
				harness.client.RequestGitIndexMutation({
					agent_id: "agent_1",
					expected_snapshot_id: snapshot_id,
					expected_workspace_version: 2,
					kind: "stage",
					paths: ["src/main.ts", "src/space and-æ.ts"],
					raw_origin: { provider: "codex", reference: "item_1" },
					run_id: "run_1",
					thread_id: "thread_1",
					workspace_id: "workspace_1",
				}),
			);
			const generated_attempt = protocol.snapshot().mutation_attempts[0]!;

			expect(generated).toMatchObject({ status: "accepted" });
			expect(generated.command_id).toBe(generated_attempt.message_id);
			expect(generated.command_id).toMatch(/^message_/u);
			expect(generated_attempt).toMatchObject({
				agent_id: "agent_1",
				kind: "git.index.stage.request",
				payload: {
					expected_snapshot_id: snapshot_id,
					expected_workspace_version: 2,
					paths: ["src/main.ts", "src/space and-æ.ts"],
					workspace_id: "workspace_1",
				},
				raw_origin: { provider: "codex", reference: "item_1" },
				run_id: "run_1",
				thread_id: "thread_1",
			});
			if (generated_attempt.kind === "git.mutation.resolve") {
				throw new Error("Expected a Git index request");
			}
			expect(generated_attempt.payload.mutation_id).toMatch(/^git_mutation_/u);
			expect(generated_attempt.payload.approval_id).toMatch(/^git_approval_/u);

			const unstage = await Effect.runPromise(
				harness.client.RequestGitIndexMutation({
					approval_id: "approval_explicit",
					command_id: "git_unstage_explicit",
					expected_snapshot_id: snapshot_id,
					expected_workspace_version: 2,
					kind: "unstage",
					mutation_id: "mutation_explicit",
					paths: ["src/main.ts"],
					thread_id: "thread_1",
					workspace_id: "workspace_1",
				}),
			);
			const resolved = await Effect.runPromise(
				harness.client.ResolveGitMutation({
					agent_id: "agent_resolver",
					approval_id: "approval_explicit",
					approved: true,
					command_id: "git_resolve_explicit",
					mutation_id: "mutation_explicit",
					raw_origin: { provider: "codex", reference: "approval_item" },
					run_id: "run_resolver",
					thread_id: "thread_1",
				}),
			);

			expect([unstage, resolved]).toMatchObject([
				{ command_id: "git_unstage_explicit", status: "accepted" },
				{ command_id: "git_resolve_explicit", status: "accepted" },
			]);
			expect(protocol.snapshot().mutation_attempts.slice(1)).toMatchObject([
				{
					kind: "git.index.unstage.request",
					message_id: "git_unstage_explicit",
					payload: {
						approval_id: "approval_explicit",
						mutation_id: "mutation_explicit",
					},
				},
				{
					agent_id: "agent_resolver",
					kind: "git.mutation.resolve",
					message_id: "git_resolve_explicit",
					payload: {
						approval_id: "approval_explicit",
						approved: true,
						mutation_id: "mutation_explicit",
					},
					raw_origin: { provider: "codex", reference: "approval_item" },
					run_id: "run_resolver",
					thread_id: "thread_1",
				},
			]);
		} finally {
			await harness.dispose();
		}
	});

	it("maps correlated Git query and receipt rejections to ArtisanClientError", async () => {
		const protocol = make_git_protocol_server();
		const harness = await make_transport_test_harness_with_protocol_server(protocol.server);

		try {
			const query_error = await Effect.runPromise(
				harness.client
					.GetGitDiff({
						expected_snapshot_id: snapshot_id,
						expected_workspace_version: 2,
						scope: "staged",
						workspace_id: "workspace_rejected",
					})
					.pipe(Effect.flip),
			);
			const mutation_error = await Effect.runPromise(
				harness.client
					.RequestGitIndexMutation({
						expected_snapshot_id: snapshot_id,
						expected_workspace_version: 2,
						kind: "stage",
						paths: ["reject.ts"],
						thread_id: "thread_1",
						workspace_id: "workspace_1",
					})
					.pipe(Effect.flip),
			);

			expect(
				[query_error, mutation_error].every((error) => error instanceof ArtisanClientError),
			).toBe(true);
			expect(query_error).toMatchObject({
				code: "protocol",
				protocol_code: "git.changed",
				retryable: false,
			});
			expect(mutation_error).toMatchObject({
				code: "protocol",
				protocol_code: "git.path_rejected",
				retryable: false,
			});
		} finally {
			await harness.dispose();
		}
	});

	it("retries the exact generated Git mutation envelope after reconnect", async () => {
		const protocol = make_git_protocol_server();
		const harness = await make_transport_test_harness_with_protocol_server(protocol.server, {
			client: { reconnect_delay_ms: 5 },
			drop_first_command_receipt: true,
		});

		try {
			const receipt = await Effect.runPromise(
				harness.client.RequestGitIndexMutation({
					expected_snapshot_id: snapshot_id,
					expected_workspace_version: 2,
					kind: "stage",
					paths: ["src/retry.ts"],
					thread_id: "thread_1",
					workspace_id: "workspace_1",
				}),
			);

			await wait_for(() => harness.connector_snapshot().connections >= 2);
			const attempts = protocol.snapshot().mutation_attempts;

			expect(receipt).toMatchObject({ status: "duplicate" });
			expect(attempts).toHaveLength(2);
			expect(attempts[1]).toEqual(attempts[0]);
			expect(receipt.command_id).toBe(attempts[0]!.message_id);
		} finally {
			await harness.dispose();
		}
	});
});

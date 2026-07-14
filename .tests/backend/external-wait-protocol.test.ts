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
	ExternalWaitDispatchScheduler,
	ExternalWaitScheduler,
	HostedGitSnapshotService,
	make_backend_runtime,
	make_git_provider_registry_layer,
	make_node_workspace_git_registry_layer,
	ProjectRepository,
	ProtocolRouter,
	ProtocolServer,
	type ProtocolConnection,
} from "@artisan/backend";
import type {
	ExternalWaitCancelEnvelope,
	ExternalWaitManualResumeEnvelope,
	ExternalWaitRequestEnvelope,
	ExternalWaitUpdatedEvent,
	HelloEnvelope,
	HostedGitCheck,
	HostedGitPullRequestLookup,
	OutboundControlEnvelope,
} from "@artisan/protocol";

import type { GitProvider } from "../../modules/backend/src/git-provider/git-provider";
import {
	GitProviderRegistry,
	GitProviderRegistryError,
} from "../../modules/backend/src/git-provider/git-provider-registry";
import { Database } from "../../modules/backend/src/persistence/database";
import { OrchestrationRuns } from "../../modules/backend/src/persistence/schema";
import {
	WorkspaceGitRegistrationError,
	WorkspaceGitRegistry,
} from "../../modules/backend/src/git/workspace-git-registry";
import { make_transport_test_harness_with_protocol_server } from "../transport/message-channel-harness";

const exec_file = promisify(execFile);
const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const protocol_time = "2026-07-14T17:00:00.000Z";
const post_client_time = "2026-07-14T23:00:00.000Z";
const manual_resume_time = "2026-07-14T23:01:00.000Z";
const thread_id = "thread_external_wait_protocol";
const project_native_id = "repository_external_wait_protocol";

interface RepositoryFixture {
	readonly database_path: string;
	readonly head: string;
	readonly root: string;
	readonly workspace_id: string;
}

interface ProviderState {
	commit_during_read?: () => Promise<void>;
	reads: number;
}

const OpenConnection = Effect.gen(function* () {
	const protocol_server = yield* ProtocolServer;

	return yield* protocol_server.Open;
});

function take_outbound(connection: ProtocolConnection, count: number) {
	return connection.Outbound.pipe(
		Stream.take(count),
		Stream.runCollect,
		Effect.timeout("5 seconds"),
	);
}

function take_until_outbound(
	connection: ProtocolConnection,
	predicate: (envelope: OutboundControlEnvelope) => boolean,
) {
	return connection.Outbound.pipe(
		Stream.takeUntil(predicate),
		Stream.runCollect,
		Effect.timeout("5 seconds"),
	);
}

function take_until_wait_state(
	connection: ProtocolConnection,
	wait_id: string,
	state_tags: ReadonlyArray<string>,
) {
	return take_until_outbound(
		connection,
		(envelope) =>
			is_external_wait_updated(envelope) &&
			envelope.payload.snapshot.wait_id === wait_id &&
			state_tags.includes(envelope.payload.snapshot.state._tag),
	);
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

async function make_repository(): Promise<RepositoryFixture> {
	const directory = await mkdtemp(join(tmpdir(), "artisan-external-wait-protocol-"));
	const root = join(directory, "repository");

	temporary_directories.push(directory);
	await mkdir(root, { recursive: true });
	await exec_file("git", ["init", "-b", "main"], { cwd: root });
	await exec_file("git", ["config", "user.email", "external-wait@example.test"], {
		cwd: root,
	});
	await exec_file("git", ["config", "user.name", "External Wait Protocol"], {
		cwd: root,
	});
	await writeFile(join(root, "waiting.txt"), "waiting\n");
	await exec_file("git", ["add", "waiting.txt"], { cwd: root });
	await exec_file("git", ["commit", "-m", "initial"], { cwd: root });

	const { stdout } = await exec_file("git", ["rev-parse", "HEAD"], { cwd: root });

	return {
		database_path: join(directory, "artisan.db"),
		head: stdout.trim(),
		root: await realpath(root),
		workspace_id: derive_workspace_id(),
	};
}

function check(): HostedGitCheck {
	return {
		annotations: [],
		annotations_truncated: false,
		name: "build",
		origin: {
			native_id: "check_external_wait_protocol",
			provider_id: "github",
			resource_kind: "check_run",
		},
		required: true,
		state: "running",
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
		ReadPullRequest: (input) =>
			Effect.promise(async () => {
				state.reads += 1;
				await state.commit_during_read?.();

				return {
					association: {
						_tag: "matched",
						freshness: "current",
						pull_request: {
							base_branch: "main",
							base_commit: "a".repeat(40),
							checks: [check()],
							checks_total: 1,
							checks_truncated: false,
							draft: false,
							head_branch: input.selected_branch,
							head_commit: input.expected_head,
							mergeability: "mergeable",
							number: 7,
							origin: {
								native_id: "pr_external_wait_protocol",
								provider_id: "github",
								resource_kind: "pull_request",
							},
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
							title: "External wait protocol",
							web_url: "https://github.com/artisan/editor/pull/7",
						},
					},
					branch: input.selected_branch,
					expected_head_commit: input.expected_head,
					repository: input.repository,
				} satisfies HostedGitPullRequestLookup;
			}),
	};
}

function make_runtime(fixture: RepositoryFixture, provider_state: ProviderState = { reads: 0 }) {
	const workspace_git_registry = make_node_workspace_git_registry_layer([
		{ root: fixture.root, workspace_id: fixture.workspace_id },
	]).pipe(Layer.provide(NodeFileSystem.layer)) as unknown as Layer.Layer<
		WorkspaceGitRegistry,
		WorkspaceGitRegistrationError
	>;
	const git_provider_registry = make_git_provider_registry_layer([
		{ hosts: ["github.com"], provider: make_provider(provider_state) },
	]) as Layer.Layer<GitProviderRegistry, GitProviderRegistryError>;
	const external_wait_scheduler = Layer.succeed(ExternalWaitScheduler, {
		Schedule: () => Effect.never,
	});
	const external_wait_dispatch_scheduler = Layer.succeed(ExternalWaitDispatchScheduler, {
		Schedule: () => Effect.never,
	});

	return make_backend_runtime({
		database_path: fixture.database_path,
		external_wait_dispatch_scheduler,
		external_wait_scheduler,
		git_provider_registry,
		migrations_path,
		workspace_git_registry,
	});
}

const Seed = (fixture: RepositoryFixture) =>
	Effect.gen(function* () {
		const database = yield* Database;
		const hosted_git_snapshots = yield* HostedGitSnapshotService;
		const projects = yield* ProjectRepository;
		const router = yield* ProtocolRouter;

		yield* router.Route({
			kind: "command",
			message_id: "create_external_wait_protocol_thread",
			origin: "frontend",
			payload: { title: "External wait protocol", type: "thread.create" },
			protocol_version: 1,
			schema_version: 1,
			sent_at: protocol_time,
			thread_id,
		});
		const registration = yield* projects.RegisterHosted({
			canonical_root: fixture.root,
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
			message_id: "assign_external_wait_protocol_project",
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
		yield* database.client.insert(OrchestrationRuns).values([
			{
				agent_id: "agent_wait_cancel",
				created_at: protocol_time,
				engine_id: "codex",
				run_id: "run_wait_cancel",
				status: "running",
				thread_id,
				updated_at: protocol_time,
				working_directory: fixture.root,
			},
			{
				agent_id: "agent_wait_manual",
				created_at: protocol_time,
				engine_id: "codex",
				run_id: "run_wait_manual",
				status: "running",
				thread_id,
				updated_at: protocol_time,
				working_directory: fixture.root,
			},
			{
				agent_id: "agent_wait_client",
				created_at: protocol_time,
				engine_id: "codex",
				run_id: "run_wait_client",
				status: "running",
				thread_id,
				updated_at: protocol_time,
				working_directory: fixture.root,
			},
			{
				agent_id: "agent_wait_typed",
				created_at: protocol_time,
				engine_id: "codex",
				run_id: "run_wait_typed",
				status: "running",
				thread_id,
				updated_at: protocol_time,
				working_directory: fixture.root,
			},
			{
				agent_id: "agent_wait_race",
				created_at: protocol_time,
				engine_id: "codex",
				run_id: "run_wait_race",
				status: "running",
				thread_id,
				updated_at: protocol_time,
				working_directory: fixture.root,
			},
		]);
		yield* hosted_git_snapshots.Refresh({
			message_id: "refresh_external_wait_protocol_snapshot",
			sent_at: protocol_time,
			thread_id,
			workspace_id: fixture.workspace_id,
		});
	});

const PrepareRuns = (updated_at: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.update(OrchestrationRuns).set({ status: "running", updated_at });
	});

function request(
	message_id: string,
	fixture: RepositoryFixture,
	source_run_id: string,
	gates: ExternalWaitRequestEnvelope["payload"]["gates"] = [{ _tag: "required_checks_terminal" }],
	sent_at = protocol_time,
): ExternalWaitRequestEnvelope {
	return {
		kind: "external_wait.request",
		message_id,
		origin: "frontend",
		payload: {
			expected_head_commit: fixture.head,
			gates,
			pull_request_number: 7,
			source_run_id,
			workspace_id: fixture.workspace_id,
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at,
		thread_id,
	};
}

function cancel(
	message_id: string,
	wait_id: string,
	sent_at = protocol_time,
): ExternalWaitCancelEnvelope {
	return {
		kind: "external_wait.cancel",
		message_id,
		origin: "frontend",
		payload: { wait_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at,
		thread_id,
	};
}

function manual_resume(message_id: string, wait_id: string): ExternalWaitManualResumeEnvelope {
	return {
		kind: "external_wait.manual_resume",
		message_id,
		origin: "frontend",
		payload: { wait_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at: manual_resume_time,
		thread_id,
	};
}

async function commit_during_provider_read(fixture: RepositoryFixture) {
	await writeFile(join(fixture.root, "race.txt"), "race\n");
	await exec_file("git", ["add", "race.txt"], { cwd: fixture.root });
	await exec_file("git", ["commit", "-m", "race"], { cwd: fixture.root });
}

function receipt(envelopes: ReadonlyArray<OutboundControlEnvelope>) {
	const result = envelopes.find((envelope) => envelope.kind === "command.receipt");

	if (result?.kind !== "command.receipt") {
		throw new Error("Expected a command receipt");
	}

	return result;
}

function is_external_wait_updated(envelope: OutboundControlEnvelope): envelope is Extract<
	OutboundControlEnvelope,
	{ readonly kind: "event" }
> & {
	readonly payload: ExternalWaitUpdatedEvent;
} {
	return envelope.kind === "event" && envelope.payload.type === "external_wait.updated";
}

function updated_event(envelopes: ReadonlyArray<OutboundControlEnvelope>, wait_id: string) {
	const result = envelopes.find(is_external_wait_updated);

	if (result === undefined || result.payload.snapshot.wait_id !== wait_id) {
		throw new Error(`Expected an external wait update for ${wait_id}`);
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

describe("external wait protocol", () => {
	it("persists, replays, cancels, and manually resumes waits through real control connections", async () => {
		const fixture = await make_repository();
		const first_runtime = make_runtime(fixture);

		await first_runtime.runPromise(Seed(fixture));
		await first_runtime.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const connection = yield* OpenConnection;

					yield* Negotiate(connection, "hello_external_wait_first");
					yield* connection.Receive(request("wait_client", fixture, "run_wait_client"));
					const accepted = yield* take_until_wait_state(connection, "wait_client", [
						"waiting",
					]);

					expect(receipt(accepted).payload).toMatchObject({ status: "accepted" });
					expect(updated_event(accepted, "wait_client").payload.snapshot.state).toEqual({
						_tag: "waiting",
					});
				}),
			),
		);
		await first_runtime.dispose();

		const second_runtime = make_runtime(fixture);
		const protocol_server = await second_runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
			client: { reconnect_delay_ms: 5 },
		});

		try {
			const waiting = await Effect.runPromise(harness.client.GetExternalWaits({ thread_id }));
			const client_cancel = await Effect.runPromise(
				harness.client.CancelExternalWait({
					command_id: "cancel_wait_client",
					thread_id,
					wait_id: "wait_client",
				}),
			);
			await second_runtime.runPromise(PrepareRuns(protocol_time));

			expect(waiting.snapshots).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						state: expect.objectContaining({
							_tag: expect.stringMatching(/^(waiting|suspended)$/u),
						}),
						wait_id: "wait_client",
					}),
				]),
			);
			expect(client_cancel.status).toBe("accepted");
			const client_request = await Effect.runPromise(
				harness.client.RequestExternalWait({
					command_id: "wait_typed",
					expected_head_commit: fixture.head,
					gates: [{ _tag: "required_checks_terminal" }],
					pull_request_number: 7,
					source_run_id: "run_wait_typed",
					thread_id,
					workspace_id: fixture.workspace_id,
				}),
			);
			const typed_waiting = await Effect.runPromise(
				harness.client.GetExternalWaits({ thread_id }),
			);

			expect(client_request).toMatchObject({
				command_id: "wait_typed",
				status: "accepted",
			});
			expect(typed_waiting.snapshots).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						state: expect.objectContaining({ _tag: "waiting" }),
						wait_id: "wait_typed",
					}),
				]),
			);
			const client_manual_resume = await Effect.runPromise(
				harness.client.ManuallyResumeExternalWait({
					command_id: "resume_wait_typed",
					thread_id,
					wait_id: "wait_typed",
				}),
			);

			const manually_resumed = await Effect.runPromise(
				harness.client.GetExternalWaits({ thread_id }),
			);

			expect(client_manual_resume).toMatchObject({
				command_id: "resume_wait_typed",
				status: "accepted",
			});
			expect(manually_resumed.snapshots).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						state: expect.objectContaining({
							_tag: expect.stringMatching(/^(wake_pending|woken)$/u),
						}),
						wait_id: "wait_typed",
					}),
				]),
			);
			await second_runtime.runPromise(PrepareRuns(post_client_time));

			await second_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* OpenConnection;

						yield* Negotiate(connection, "hello_external_wait_second");
						yield* connection.Receive(
							request("wait_client", fixture, "run_wait_client"),
						);
						const replay = yield* take_outbound(connection, 1);

						expect(receipt(replay).payload).toMatchObject({ status: "duplicate" });

						yield* connection.Receive(
							request("wait_client", fixture, "run_wait_client", [
								{ _tag: "review_decision_changed" },
							]),
						);
						const changed = yield* take_outbound(connection, 1);

						expect(receipt(changed).payload).toMatchObject({
							error: { code: expect.stringMatching(/^external_wait\./u) },
							status: "rejected",
						});

						yield* connection.Receive(
							request(
								"wait_cancel",
								fixture,
								"run_wait_cancel",
								undefined,
								post_client_time,
							),
						);
						const accepted = yield* take_until_wait_state(connection, "wait_cancel", [
							"waiting",
						]);

						expect(receipt(accepted).payload).toMatchObject({
							status: "accepted",
						});

						yield* connection.Receive(
							cancel("cancel_wait", "wait_cancel", post_client_time),
						);
						const cancelled = yield* take_until_wait_state(connection, "wait_cancel", [
							"cancelled",
						]);

						expect(receipt(cancelled).payload).toMatchObject({ status: "accepted" });
						expect(
							updated_event(cancelled, "wait_cancel").payload.snapshot.state,
						).toEqual({
							_tag: "cancelled",
							reason: "user",
						});

						yield* connection.Receive(
							cancel("cancel_wait", "wait_cancel", post_client_time),
						);
						const cancelled_replay = yield* take_outbound(connection, 1);

						expect(receipt(cancelled_replay).payload).toMatchObject({
							status: "duplicate",
						});
					}),
				),
			);
			await second_runtime.runPromise(PrepareRuns(manual_resume_time));
			await second_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* OpenConnection;

						yield* Negotiate(connection, "hello_external_wait_manual");
						yield* connection.Receive(
							request(
								"wait_manual",
								fixture,
								"run_wait_manual",
								undefined,
								manual_resume_time,
							),
						);
						yield* take_until_wait_state(connection, "wait_manual", ["waiting"]);
						yield* connection.Receive(manual_resume("resume_wait", "wait_manual"));
						const resumed = yield* take_until_wait_state(connection, "wait_manual", [
							"wake_pending",
							"woken",
						]);

						expect(receipt(resumed).payload).toMatchObject({ status: "accepted" });
						expect(
							updated_event(resumed, "wait_manual").payload.snapshot.state,
						).toMatchObject({
							_tag: expect.stringMatching(/^(wake_pending|woken)$/u),
						});

						yield* connection.Receive(manual_resume("resume_wait", "wait_manual"));
						const resumed_replay = yield* take_outbound(connection, 1);

						expect(receipt(resumed_replay).payload).toMatchObject({
							status: "duplicate",
						});
					}),
				),
			);
		} finally {
			await harness.dispose();
			await second_runtime.dispose();
		}
	}, 30_000);

	it("rejects a typed request when a real provider-read commit changes the exact head", async () => {
		const fixture = await make_repository();
		const provider_state: ProviderState = { reads: 0 };
		const runtime = make_runtime(fixture, provider_state);

		await runtime.runPromise(Seed(fixture));
		provider_state.commit_during_read = () => commit_during_provider_read(fixture);

		const protocol_server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
			client: { reconnect_delay_ms: 5 },
		});

		try {
			await expect(
				Effect.runPromise(
					harness.client.RequestExternalWait({
						command_id: "wait_race",
						expected_head_commit: fixture.head,
						gates: [{ _tag: "required_checks_terminal" }],
						pull_request_number: 7,
						source_run_id: "run_wait_race",
						thread_id,
						workspace_id: fixture.workspace_id,
					}),
				),
			).rejects.toMatchObject({
				code: "protocol",
				protocol_code: "external_wait.branch_changed",
				retryable: true,
			});
			const waits = await Effect.runPromise(harness.client.GetExternalWaits({ thread_id }));
			const runs = await runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client
						.select({
							run_id: OrchestrationRuns.run_id,
							status: OrchestrationRuns.status,
						})
						.from(OrchestrationRuns),
				),
			);

			expect(
				waits.snapshots.find((snapshot) => snapshot.wait_id === "wait_race"),
			).toBeUndefined();
			expect(runs.find((run) => run.run_id === "run_wait_race")).toEqual({
				run_id: "run_wait_race",
				status: "running",
			});
			expect(provider_state.reads).toBe(2);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	}, 30_000);
});

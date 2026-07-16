import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	GitProvider,
	HostedGitMutationCoordinator,
	make_backend_runtime,
	make_git_provider_registry_layer,
	make_node_workspace_git_registry_layer,
	ProjectRepository,
	ProtocolRouter,
	ProtocolServer,
	type WorkspaceGitRegistrationError,
	type WorkspaceGitRegistry,
} from "@artisan/backend";

import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

import {
	make_transport_test_harness_with_protocol_server,
	wait_for,
} from "./message-channel-harness";

const exec_file = promisify(execFile);
const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const thread_id = "thread_hosted_git_mutation_transport";
const provider_id = "github";
const repository = {
	host: "github.com",
	name: "editor",
	owner: "artisan",
	provider_id,
};
const selection = { account_login: "alice", host: "github.com", provider_id };
const pull_request_origin = {
	native_id: "PR_42",
	provider_id,
	resource_kind: "pull_request" as const,
};
const review_thread_origin = {
	native_id: "RT_7",
	provider_id,
	resource_kind: "review_thread" as const,
};

interface ProviderState {
	provider_calls: number;
}

function derive_workspace_id() {
	const parts = [provider_id, "github.com", "repository_1"].map((part) =>
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
	const directory = await mkdtemp(join(tmpdir(), "artisan-hosted-git-mutation-transport-"));
	const root = join(directory, "repository");

	directories.push(directory);
	await mkdir(root, { recursive: true });
	await exec_file("git", ["init", "-b", "main"], { cwd: root });
	await exec_file("git", ["config", "user.email", "transport@example.test"], { cwd: root });
	await exec_file("git", ["config", "user.name", "Transport Test"], { cwd: root });
	await exec_file("git", ["remote", "add", "origin", "https://github.com/artisan/editor.git"], {
		cwd: root,
	});
	await writeFile(join(root, "accepted.txt"), "main\n");
	await exec_file("git", ["add", "accepted.txt"], { cwd: root });
	await exec_file("git", ["commit", "-m", "initial"], { cwd: root });
	const { stdout } = await exec_file("git", ["rev-parse", "HEAD"], { cwd: root });

	return {
		database_path: join(directory, "artisan.db"),
		head: stdout.trim(),
		root: await realpath(root),
		workspace_id: derive_workspace_id(),
	};
}

function make_metadata_layer(instance_id: string) {
	let next_id = 0;
	let next_time = Date.parse("2026-07-16T16:00:00.000Z");

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++next_id}`),
		Now: Effect.sync(() => new Date(next_time++).toISOString()),
	});
}

function make_provider(state: ProviderState): typeof GitProvider.Service {
	return {
		Clone: () => Effect.die("Clone is outside hosted Git mutation transport tests"),
		Descriptor: {
			capabilities: [
				{ _tag: "available", capability: "read_reviews" },
				{ _tag: "available", capability: "read_ci" },
				{ _tag: "available", capability: "write_provider_mutations" },
			],
			display_name: "GitHub",
			provider_id,
		},
		DiscoverRepositories: () =>
			Effect.die("Discovery is outside hosted Git mutation transport tests"),
		ExecuteMutation: (input) =>
			Effect.sync(() => {
				state.provider_calls += 1;

				return {
					operation: "reply_review_thread" as const,
					origin: {
						native_id: `RC_${input.client_mutation_id}`,
						provider_id,
						resource_kind: "review_comment" as const,
					},
					status: "applied" as const,
					thread_origin: review_thread_origin,
				};
			}),
		Inspect: Effect.die("Inspection is outside hosted Git mutation transport tests"),
		PrepareClone: () =>
			Effect.die("Preparation is outside hosted Git mutation transport tests"),
		ReadPullRequest: (input) =>
			Effect.succeed({
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
						head_branch: input.selected_branch,
						head_commit: input.expected_head,
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
								subject: "line" as const,
							},
						],
						review_threads_total: 1,
						review_threads_truncated: false,
						reviews: [],
						reviews_total: 0,
						reviews_truncated: false,
						state: "open" as const,
						title: "Transport mutation target",
						web_url: "https://github.com/artisan/editor/pull/42",
					},
				},
				branch: input.selected_branch,
				expected_head_commit: input.expected_head,
				repository: input.repository,
			}),
	};
}

async function start_stack(options: {
	readonly database_path: string;
	readonly drop_first_command_receipt?: boolean;
	readonly instance_id: string;
	readonly provider: typeof GitProvider.Service;
	readonly root: string;
	readonly workspace_id: string;
}) {
	const workspace_git_registry = make_node_workspace_git_registry_layer([
		{ root: options.root, workspace_id: options.workspace_id },
	]).pipe(Layer.provide(NodeFileSystem.layer)) as unknown as Layer.Layer<
		WorkspaceGitRegistry,
		WorkspaceGitRegistrationError
	>;
	const runtime = make_backend_runtime({
		database_path: options.database_path,
		git_provider_registry: make_git_provider_registry_layer([
			{ hosts: ["github.com"], provider: options.provider },
		]),
		migrations_path,
		runtime_metadata: make_metadata_layer(options.instance_id),
		workspace_git_registry,
	});
	const protocol_server = await runtime.runPromise(ProtocolServer);
	const coordinator = await runtime.runPromise(HostedGitMutationCoordinator);
	const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
		client: { reconnect_delay_ms: 5 },
		...(options.drop_first_command_receipt === undefined
			? {}
			: { drop_first_command_receipt: options.drop_first_command_receipt }),
	});

	return { coordinator, harness, runtime };
}

const register_and_attach_project = (root: string) =>
	Effect.gen(function* () {
		const projects = yield* ProjectRepository;
		const router = yield* ProtocolRouter;
		const registration = yield* projects.RegisterHosted({
			canonical_root: root,
			display_name: "Artisan Editor",
			hosted_origin: {
				canonical_host: "github.com",
				clone_url: "https://github.com/artisan/editor.git",
				fetch_url: "https://github.com/artisan/editor.git",
				name: "editor",
				native_id: "repository_1",
				owner: "artisan",
				provider_id,
				push_url: "https://github.com/artisan/editor.git",
				remote_name: "origin",
				selected_account_login: "alice",
				web_url: "https://github.com/artisan/editor",
			},
		});

		yield* router.Route({
			kind: "command",
			message_id: "attach_hosted_git_mutation_transport_project",
			origin: "frontend",
			payload: { project: registration.project.project, type: "thread.project.assign" },
			protocol_version: 1,
			schema_version: 1,
			sent_at: "2026-07-16T16:00:00.000Z",
			thread_id,
		});
	});

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("ArtisanClient hosted Git mutation with the backend ProtocolServer", () => {
	it("restores and retries one approved hosted mutation without exposing its private reply", async () => {
		const fixture = await make_repository();
		const state = { provider_calls: 0 };
		const provider = make_provider(state);
		let first: Awaited<ReturnType<typeof start_stack>> | undefined = await start_stack({
			database_path: fixture.database_path,
			instance_id: "before_restart",
			provider,
			root: fixture.root,
			workspace_id: fixture.workspace_id,
		});
		let second: Awaited<ReturnType<typeof start_stack>> | undefined;

		try {
			await Effect.runPromise(
				first.harness.client.Command({
					command_id: "create_hosted_git_mutation_transport_thread",
					payload: { title: "Hosted mutation transport", type: "thread.create" },
					thread_id,
				}),
			);
			await first.runtime.runPromise(register_and_attach_project(fixture.root));
			await Effect.runPromise(
				first.harness.client.RefreshWorkspaceGitSession({
					command_id: "ready_hosted_git_mutation_workspace",
					thread_id,
					workspace_id: fixture.workspace_id,
				}),
			);
			const session = await Effect.runPromise(
				first.harness.client.GetWorkspaceGitSession({ workspace_id: fixture.workspace_id }),
			);
			const snapshot_receipt = await Effect.runPromise(
				first.harness.client.RefreshHostedGitSnapshot({
					command_id: "refresh_hosted_git_mutation_snapshot",
					thread_id,
					workspace_id: fixture.workspace_id,
				}),
			);
			const snapshot = await Effect.runPromise(
				first.harness.client.GetHostedGitSnapshot({ workspace_id: fixture.workspace_id }),
			);

			expect(session.session).toMatchObject({ state: "ready" });
			expect(snapshot_receipt.status).toBe("accepted");
			expect(snapshot.snapshot).toMatchObject({ workspace_freshness: "current" });

			if (!snapshot.snapshot) {
				throw new Error("Expected a current hosted Git snapshot");
			}

			const requested_receipt = await Effect.runPromise(
				first.harness.client.RequestHostedGitMutation({
					command_id: "request_hosted_git_mutation_transport",
					mutation: {
						body: "Private review reply that must not enter the public projection",
						expected_head_commit: snapshot.snapshot.lookup.expected_head_commit,
						operation: "reply_review_thread",
						pull_request_number: 42,
						pull_request_origin,
						repository,
						selected_branch: snapshot.snapshot.lookup.branch,
						snapshot_version: snapshot.snapshot.version,
						thread_origin: review_thread_origin,
						workspace_id: fixture.workspace_id,
					},
					selection,
					thread_id,
				}),
			);
			const approval_id = "hosted_git_mutation:request_hosted_git_mutation_transport";
			const requested = await Effect.runPromise(
				first.harness.client.GetHostedGitMutationApproval({ approval_id, thread_id }),
			);

			expect(requested_receipt.status).toBe("accepted");
			expect(requested.approval).toMatchObject({ state: "requested" });
			expect(requested.approval.operation).not.toHaveProperty("body");
			expect(requested.approval).not.toHaveProperty("provider_output");

			await first.harness.dispose();
			await first.runtime.dispose();
			first = undefined;

			second = await start_stack({
				database_path: fixture.database_path,
				drop_first_command_receipt: true,
				instance_id: "after_restart",
				provider,
				root: fixture.root,
				workspace_id: fixture.workspace_id,
			});
			const restored = await Effect.runPromise(
				second.harness.client.GetHostedGitMutationApproval({ approval_id, thread_id }),
			);

			expect(restored.approval).toMatchObject({ state: "requested" });

			const approval_promise = Effect.runPromise(
				second.harness.client.RespondHostedGitMutationApproval({
					approval_id,
					approved: true,
					command_id: "approve_hosted_git_mutation_transport",
					thread_id,
				}),
			);

			await wait_for(
				() => second!.harness.connector_snapshot().dropped_command_receipts === 1,
			);
			second.harness.close_current_connection();
			await wait_for(() => second!.harness.connector_snapshot().connections >= 2);

			const approval_receipt = await approval_promise;

			await second.runtime.runPromise(second.coordinator.AwaitIdle);
			const applied = await Effect.runPromise(
				second.harness.client.GetHostedGitMutationApproval({ approval_id, thread_id }),
			);

			expect(approval_receipt).toMatchObject({
				command_id: "approve_hosted_git_mutation_transport",
				status: "duplicate",
			});
			expect(applied.approval).toMatchObject({ state: "applied" });
			expect(applied.approval.operation).not.toHaveProperty("body");
			expect(applied.approval).not.toHaveProperty("provider_output");
			expect(state.provider_calls).toBe(1);
		} finally {
			await first?.harness.dispose();
			await first?.runtime.dispose();
			await second?.harness.dispose();
			await second?.runtime.dispose();
		}
	});
});

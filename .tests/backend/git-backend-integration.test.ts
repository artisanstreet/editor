import { execFile } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { GitWorkspaceQueryEnvelope } from "@artisan/protocol";
import {
	GitService,
	make_backend_runtime,
	make_node_workspace_git_registry_layer,
	ProtocolServer,
} from "@artisan/backend";

import { make_transport_test_harness_with_protocol_server } from "../transport/message-channel-harness";

const exec_file = promisify(execFile);
const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const sent_at = "2026-07-18T16:00:00.000Z";

async function run_git(root: string, args: ReadonlyArray<string>) {
	return exec_file("git", args, { cwd: root, encoding: "utf8", windowsHide: true });
}

async function make_fixture() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-git-backend-"));
	const root = join(directory, "repository");

	temporary_directories.push(directory);
	await mkdir(root);
	await run_git(root, ["init", "-q"]);
	await run_git(root, ["config", "user.email", "artisan@example.invalid"]);
	await run_git(root, ["config", "user.name", "Artisan Test"]);
	await writeFile(join(root, "tracked.txt"), "before\n");
	await run_git(root, ["add", "tracked.txt"]);
	await run_git(root, ["commit", "-qm", "initial"]);
	await writeFile(join(root, "tracked.txt"), "after\n");

	return {
		database_path: join(directory, "artisan.db"),
		root,
	};
}

const query = (message_id: string): GitWorkspaceQueryEnvelope => ({
	kind: "git.workspace.query",
	message_id,
	origin: "frontend",
	payload: { thread_id: "thread_git", workspace_id: "workspace_git" },
	protocol_version: 1,
	schema_version: 1,
	sent_at,
});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("production Git backend integration", () => {
	it("stages through the Git CLI once and preserves the durable projection across restart", async () => {
		const fixture = await make_fixture();
		const make_runtime = () =>
			make_backend_runtime({
				database_path: fixture.database_path,
				migrations_path,
				workspace_git_registry: make_node_workspace_git_registry_layer([
					{ root: fixture.root, workspace_id: "workspace_git" },
				]),
			});
		const first_runtime = make_runtime();
		let transport:
			| Awaited<ReturnType<typeof make_transport_test_harness_with_protocol_server>>
			| undefined;

		try {
			const protocol_server = await first_runtime.runPromise(ProtocolServer);
			transport = await make_transport_test_harness_with_protocol_server(protocol_server);
			const result = await Effect.runPromise(
				Effect.gen(function* () {
					yield* transport!.client.Command({
						command_id: "create_thread",
						payload: { title: "Git integration", type: "thread.create" },
						thread_id: "thread_git",
					});
					const before = yield* transport!.client.GetGitWorkspace({
						thread_id: "thread_git",
						workspace_id: "workspace_git",
					});

					if (before.workspace.repository_state !== "repository") {
						return yield* Effect.die("Expected a Git repository projection");
					}

					const requested = yield* transport!.client.RequestGitIndexMutation({
						approval_id: "approval_stage",
						command_id: "request_stage",
						expected_snapshot_id: before.workspace.snapshot_id,
						expected_workspace_version: before.workspace.version,
						kind: "stage",
						mutation_id: "mutation_stage",
						paths: ["tracked.txt"],
						thread_id: "thread_git",
						workspace_id: "workspace_git",
					});
					const pending = yield* transport!.client.GetGitWorkspace({
						thread_id: "thread_git",
						workspace_id: "workspace_git",
					});
					const resolution = {
						approval_id: "approval_stage",
						approved: true,
						command_id: "resolve_stage",
						mutation_id: "mutation_stage",
						thread_id: "thread_git",
					} as const;
					const resolved = yield* transport!.client.ResolveGitMutation(resolution);
					const after = yield* transport!.client.GetGitWorkspace({
						thread_id: "thread_git",
						workspace_id: "workspace_git",
					});

					return { after, before, pending, requested, resolved };
				}),
			);
			const cached = await run_git(fixture.root, ["diff", "--cached", "--name-only"]);

			expect(result.before.workspace).toMatchObject({
				repository_state: "repository",
				version: 1,
			});
			expect(result.requested.status).toBe("accepted");
			expect(result.pending.pending_mutations).toMatchObject([
				{ lifecycle: "awaiting_approval", mutation_id: "mutation_stage" },
			]);
			expect(result.resolved.status).toBe("accepted");
			expect(result.after.workspace).toMatchObject({
				repository_state: "repository",
				version: 2,
			});
			expect(cached.stdout.trim()).toBe("tracked.txt");
		} finally {
			await transport?.dispose();
			await first_runtime.dispose();
		}

		const restarted = make_runtime();

		try {
			const after_restart = await restarted.runPromise(
				Effect.flatMap(GitService, (git) => git.Query(query("query_restart"))),
			);
			const cached = await run_git(fixture.root, ["diff", "--cached", "--name-only"]);

			expect(after_restart.workspace).toMatchObject({
				repository_state: "repository",
				version: 2,
			});
			expect(after_restart.pending_mutations).toEqual([]);
			expect(cached.stdout.trim()).toBe("tracked.txt");
		} finally {
			await restarted.dispose();
		}
	});
});

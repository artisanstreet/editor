import { promises as fs } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	GitCommandExecutor,
	type GitCommandInput,
	type GitCommandResult,
} from "../../modules/backend/src/git/executor";
import {
	make_workspace_git_registry_layer,
	WorkspaceGitRegistry,
} from "../../modules/backend/src/git/workspace-git-registry";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan-workspace-git-registry-"));

	roots.push(root);

	return root;
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

function make_layer(
	registrations: ReadonlyArray<unknown>,
	run: (input: GitCommandInput) => Effect.Effect<GitCommandResult> = () =>
		Effect.succeed({
			exit_code: 0,
			stderr: { bytes: new Uint8Array(), total_bytes: 0, truncated: false },
			stdout: { bytes: new Uint8Array(), total_bytes: 0, truncated: false },
		}),
) {
	return make_workspace_git_registry_layer(registrations).pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(Layer.succeed(GitCommandExecutor, { Run: run })),
	);
}

describe("WorkspaceGitRegistry", () => {
	it("binds an opaque workspace to its canonical root and rejects child authorization", async () => {
		const root = await make_root();
		const child = join(root, "child");
		const invocations: Array<GitCommandInput> = [];

		await fs.mkdir(child);

		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* WorkspaceGitRegistry;
				const capability = yield* registry.Get("workspace_one");

				yield* capability.git.Run({
					args: ["status"],
					max_stderr_bytes: 1,
					max_stdin_bytes: 0,
					max_stdout_bytes: 1,
					mode: "read",
				});

				const authorized = yield* registry.Authorize({
					working_directory: root,
					workspace_id: "workspace_one",
				});
				const rejected = yield* registry
					.Authorize({ working_directory: child, workspace_id: "workspace_one" })
					.pipe(Effect.flip);

				return { authorized, rejected };
			}).pipe(
				Effect.provide(
					make_layer([{ root, workspace_id: "workspace_one" }], (input) => {
						invocations.push(input);

						return Effect.succeed({
							exit_code: 0,
							stderr: { bytes: new Uint8Array(), total_bytes: 0, truncated: false },
							stdout: { bytes: new Uint8Array(), total_bytes: 0, truncated: false },
						});
					}),
				),
			),
		);

		expect(result.authorized.workspace_id).toBe("workspace_one");
		expect(result.rejected._tag).toBe("WorkspaceGitAuthorizationError");
		expect(invocations).toHaveLength(1);
		expect(invocations[0]?.cwd).toBe(result.authorized.git.root);
	});

	it("fails a command after a registered symlink is retargeted", async () => {
		const root = await make_root();
		const first = join(root, "first");
		const second = join(root, "second");
		const link = join(root, "workspace-link");

		await fs.mkdir(first);
		await fs.mkdir(second);
		await fs.symlink(first, link, process.platform === "win32" ? "junction" : "dir");

		const registry = await Effect.runPromise(
			Effect.service(WorkspaceGitRegistry).pipe(
				Effect.provide(make_layer([{ root: link, workspace_id: "workspace_one" }])),
			),
		);
		const capability = await Effect.runPromise(registry.Get("workspace_one"));

		await fs.rm(link, { force: true });
		await fs.symlink(second, link, process.platform === "win32" ? "junction" : "dir");

		const error = await Effect.runPromise(
			capability.git
				.Run({
					args: ["status"],
					max_stderr_bytes: 1,
					max_stdin_bytes: 0,
					max_stdout_bytes: 1,
					mode: "read",
				})
				.pipe(Effect.flip),
		);

		expect(error._tag).toBe("WorkspaceGitRootChangedError");
	});

	it("recognizes filesystem aliases as the current canonical root", async () => {
		const root = await make_root();
		const workspace = join(root, "workspace");
		const alias = join(root, "workspace-alias");

		await fs.mkdir(workspace);
		await fs.symlink(workspace, alias, process.platform === "win32" ? "junction" : "dir");

		const capability = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* WorkspaceGitRegistry;

				return yield* registry.Get("workspace_one");
			}).pipe(
				Effect.provide(make_layer([{ root: workspace, workspace_id: "workspace_one" }])),
			),
		);

		expect(await Effect.runPromise(capability.git.IsCurrentRoot(alias))).toBe(true);
		expect(await Effect.runPromise(capability.git.IsCurrentRoot(root))).toBe(false);
		expect(await Effect.runPromise(capability.git.IsCurrentRoot(join(root, "missing")))).toBe(
			false,
		);
	});

	it("rejects canonical root aliases during construction", async () => {
		const root = await make_root();
		const alias = join(root, "alias");

		await fs.symlink(root, alias, process.platform === "win32" ? "junction" : "dir");

		const error = await Effect.runPromise(
			Effect.service(WorkspaceGitRegistry).pipe(
				Effect.provide(
					make_layer([
						{ root, workspace_id: "workspace_one" },
						{ root: alias, workspace_id: "workspace_two" },
					]),
				),
				Effect.flip,
			),
		);

		expect(error.reason).toBe("aliased_root");
	});
});

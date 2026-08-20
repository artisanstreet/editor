import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import { type GitCommandResult } from "../../modules/backend/src/git/executor";
import {
	GitMutationDriver,
	GitMutationDriverLive,
} from "../../modules/backend/src/git/mutation-driver";
import {
	type WorkspaceGitCommandInput,
	WorkspaceGitRegistry,
} from "../../modules/backend/src/git/workspace-git-registry";

const encoder = new TextEncoder();
const oid = "1".repeat(40);

function result(stdout = ""): GitCommandResult {
	const bytes = encoder.encode(stdout);

	return {
		exit_code: 0,
		stderr: { bytes: new Uint8Array(), total_bytes: 0, truncated: false },
		stdout: { bytes, total_bytes: bytes.byteLength, truncated: false },
	};
}

function make_driver(run: (input: WorkspaceGitCommandInput) => Effect.Effect<GitCommandResult>) {
	const capability = {
		git: {
			IsCurrentRoot: (path: string) => Effect.succeed(path === "C:/repository"),
			root: "C:/repository",
			Run: run,
		},
		workspace_id: "workspace_one",
	};
	const registry = Layer.succeed(WorkspaceGitRegistry, {
		Authorize: () => Effect.succeed(capability),
		Get: () => Effect.succeed(capability),
		ListWorkspaceIds: Effect.succeed(["workspace_one"]),
		Reconcile: () => Effect.succeed([]),
		Register: () => Effect.succeed({ workspace_id: "workspace_one" }),
	});

	return GitMutationDriverLive.pipe(Layer.provide(registry));
}

describe("GitMutationDriver", () => {
	it("stages exact literal NUL-delimited paths through stdin", async () => {
		const invocations: Array<WorkspaceGitCommandInput> = [];
		const driver = await Effect.runPromise(
			Effect.service(GitMutationDriver).pipe(
				Effect.provide(
					make_driver((input) => {
						invocations.push(input);

						return Effect.succeed(result());
					}),
				),
			),
		);

		await Effect.runPromise(
			driver.Stage({
				paths: ["--leading option.txt", "odd\nname.txt"],
				workspace_id: "workspace_one",
			}),
		);

		expect(invocations).toHaveLength(1);
		expect(invocations[0]?.args).toEqual([
			"--literal-pathspecs",
			"add",
			"--pathspec-from-file=-",
			"--pathspec-file-nul",
		]);
		expect(new TextDecoder().decode(invocations[0]?.stdin)).toBe(
			"--leading option.txt\0odd\nname.txt\0",
		);
		expect(invocations[0]?.mode).toBe("mutation");
	});

	it("checks HEAD before using the exact restore --staged pathspec invocation", async () => {
		const invocations: Array<WorkspaceGitCommandInput> = [];
		const driver = await Effect.runPromise(
			Effect.service(GitMutationDriver).pipe(
				Effect.provide(
					make_driver((input) => {
						invocations.push(input);

						return Effect.succeed(
							input.args.includes("status")
								? result(`# branch.oid ${oid}\0# branch.head main\0`)
								: result(),
						);
					}),
				),
			),
		);

		await Effect.runPromise(
			driver.Unstage({ paths: ["file with spaces.txt"], workspace_id: "workspace_one" }),
		);

		expect(invocations).toHaveLength(2);
		expect(invocations[1]?.args).toEqual([
			"--literal-pathspecs",
			"restore",
			"--staged",
			"--pathspec-from-file=-",
			"--pathspec-file-nul",
		]);
		expect(new TextDecoder().decode(invocations[1]?.stdin)).toBe("file with spaces.txt\0");
	});

	it("rejects unstage on an unborn branch before invoking restore", async () => {
		const invocations: Array<WorkspaceGitCommandInput> = [];
		const driver = await Effect.runPromise(
			Effect.service(GitMutationDriver).pipe(
				Effect.provide(
					make_driver((input) => {
						invocations.push(input);

						return Effect.succeed(
							result("# branch.oid (initial)\0# branch.head new-branch\0"),
						);
					}),
				),
			),
		);
		const error = await Effect.runPromise(
			driver
				.Unstage({ paths: ["file.txt"], workspace_id: "workspace_one" })
				.pipe(Effect.flip),
		);

		expect(error.reason).toBe("unborn_head");
		expect(invocations).toHaveLength(1);
		expect(invocations[0]?.args).toContain("status");
	});
});

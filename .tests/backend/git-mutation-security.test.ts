import { spawn } from "node:child_process";
import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";

import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { GitMutation } from "../../modules/backend/src/git/git-mutation";
import {
	make_git_mutation_layer,
	make_node_git_mutation_layer,
} from "../../modules/backend/src/git/node-git-mutation";
import { make_node_process_runner_layer } from "../../modules/backend/src/git/node-process-runner";
import {
	ProcessRunner,
	type ProcessRunnerInput,
} from "../../modules/backend/src/git/process-runner";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan-git-mutation-security-"));

	roots.push(root);

	return root;
}

async function run_process(
	cwd: string,
	command: ReadonlyArray<string>,
	allowed_exit_codes: ReadonlyArray<number> = [0],
) {
	const [file, ...args] = command;

	return new Promise<{
		readonly exit_code: number;
		readonly stderr: string;
		readonly stdout: string;
	}>((resolve, reject) => {
		const child = spawn(file!, args, {
			cwd,
			shell: false,
			stdio: ["ignore", "pipe", "pipe"],
		});
		const stderr: Array<Buffer> = [];
		const stdout: Array<Buffer> = [];

		child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
		child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
		child.on("error", reject);
		child.on("close", (code) => {
			const exit_code = code ?? -1;
			const output = {
				exit_code,
				stderr: Buffer.concat(stderr).toString("utf8"),
				stdout: Buffer.concat(stdout).toString("utf8"),
			};

			if (allowed_exit_codes.includes(exit_code)) {
				resolve(output);

				return;
			}

			reject(new Error(`${command.join(" ")} exited ${exit_code}: ${output.stderr}`));
		});
	});
}

async function run_git(
	cwd: string,
	args: ReadonlyArray<string>,
	allowed_exit_codes: ReadonlyArray<number> = [0],
) {
	return run_process(cwd, ["git", ...args], allowed_exit_codes);
}

async function read_git(root: string, args: ReadonlyArray<string>) {
	return (await run_git(root, args)).stdout.trim();
}

async function initialize_repository(root: string) {
	await run_git(root, ["init", "--quiet"]);
	await run_git(root, ["config", "core.autocrlf", "false"]);
	await run_git(root, ["config", "user.email", "tests@example.com"]);
	await run_git(root, ["config", "user.name", "Artisan Tests"]);
	await fs.writeFile(join(root, "tracked.txt"), "base\n");
	await run_git(root, ["add", "tracked.txt"]);
	await run_git(root, ["commit", "--quiet", "--message", "base"]);
}

async function commit_file(root: string, path: string, content: string, message: string) {
	await fs.writeFile(join(root, path), content);
	await run_git(root, ["add", "--", path]);
	await run_git(root, ["commit", "--quiet", "--message", message]);

	return read_git(root, ["rev-parse", "HEAD"]);
}

async function make_mutation(root: string) {
	return Effect.runPromise(
		Effect.service(GitMutation).pipe(
			Effect.provide(make_node_git_mutation_layer({ cwd: root })),
		),
	);
}

async function make_hooked_mutation(
	root: string,
	hooks: { readonly before?: (input: ProcessRunnerInput) => Promise<void> },
) {
	const base_runner = await Effect.runPromise(
		Effect.service(ProcessRunner).pipe(Effect.provide(make_node_process_runner_layer())),
	);
	const runner = Layer.succeed(ProcessRunner, {
		Run: (input: ProcessRunnerInput) =>
			Effect.gen(function* () {
				if (hooks.before !== undefined) {
					yield* Effect.promise(() => hooks.before!(input));
				}

				return yield* base_runner.Run(input);
			}),
	});
	const layer = make_git_mutation_layer({ cwd: root }).pipe(
		Layer.provide(runner),
		Layer.provide(NodeCrypto.layer),
		Layer.provide(NodeFileSystem.layer),
		Layer.provide(NodePath.layer),
	);

	return Effect.runPromise(Effect.service(GitMutation).pipe(Effect.provide(layer)));
}

function shell_argument(value: string) {
	return `"${value.replaceAll("\\", "/").replaceAll('"', '\\"')}"`;
}

async function configure_executable_repository_settings(
	root: string,
	marker: string,
	scope: "--local" | "--worktree" = "--local",
) {
	const script = join(root, "record-config-invocation.mjs");
	const command = `${shell_argument(process.execPath)} ${shell_argument(script)} ${shell_argument(marker)}`;

	await fs.writeFile(
		script,
		[
			'import { appendFileSync } from "node:fs";',
			"",
			"appendFileSync(process.argv[2], `${process.argv[3]}\\n`);",
			'if (process.argv[3] === "fsmonitor") {',
			"\tprocess.stdin.resume();",
			'\tprocess.stdout.write("token\\0");',
			"} else {",
			"\tprocess.stdin.pipe(process.stdout);",
			"}",
			"",
		].join("\n"),
	);
	await run_git(root, ["config", scope, "core.fsmonitor", `${command} fsmonitor`]);
	await run_git(root, ["config", scope, "filter.adversarial.clean", `${command} clean`]);
	await run_git(root, ["config", scope, "filter.adversarial.smudge", `${command} smudge`]);
	await run_git(root, ["config", scope, "filter.adversarial.required", "true"]);
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("GitMutation adversarial repository safety", () => {
	it("pins every Git process to an absolute executable outside the workspace", async () => {
		const root = await make_root();
		const commands: Array<string> = [];

		await initialize_repository(root);

		const mutation = await make_hooked_mutation(root, {
			before: async (input) => {
				commands.push(input.command);
			},
		});

		await Effect.runPromise(mutation.Prepare({ branch: "feature", type: "branch_create" }));

		expect(commands.length).toBeGreaterThan(0);
		expect(commands.every((command) => isAbsolute(command))).toBe(true);
		expect(
			commands.every(
				(command) =>
					!command.toLocaleLowerCase("en-US").startsWith(root.toLocaleLowerCase("en-US")),
			),
		).toBe(true);
	});

	it("rejects a remote whose push URL differs from its approved fetch URL", async () => {
		const root = await make_root();
		const approved_remote = join(root, "approved.git");
		const unapproved_remote = join(root, "unapproved.git");
		const workspace = join(root, "workspace");

		await run_git(root, ["init", "--bare", "--quiet", approved_remote]);
		await run_git(root, ["init", "--bare", "--quiet", unapproved_remote]);
		await fs.mkdir(workspace);
		await initialize_repository(workspace);
		await run_git(workspace, ["remote", "add", "origin", approved_remote]);
		await run_git(workspace, ["config", "--local", "remote.origin.pushurl", unapproved_remote]);

		const mutation = await make_mutation(workspace);
		const failure = await Effect.runPromise(
			mutation
				.Prepare({
					remote: "origin",
					set_upstream: false,
					target_branch: "published",
					type: "push",
				})
				.pipe(Effect.flip),
		);

		expect(failure.operation).toBe("prepare");
		expect(await read_git(approved_remote, ["for-each-ref", "refs/heads/published"])).toBe("");
		expect(await read_git(unapproved_remote, ["for-each-ref", "refs/heads/published"])).toBe(
			"",
		);
	});

	it("rejects remote endpoints that embed credentials before network access", async () => {
		const root = await make_root();

		await initialize_repository(root);
		await run_git(root, [
			"remote",
			"add",
			"origin",
			"https://token@example.com/repository.git",
		]);

		const mutation = await make_mutation(root);
		const failure = await Effect.runPromise(
			mutation
				.Prepare({
					remote: "origin",
					set_upstream: false,
					target_branch: "published",
					type: "push",
				})
				.pipe(Effect.flip),
		);

		expect(failure.operation).toBe("prepare");
	});

	it.each([
		["http.https://example.com.sslVerify", "false"],
		["http.https://example.com.proxy", "https://attacker.example"],
	])("rejects URL-scoped transport config %s", async (key, value) => {
		const root = await make_root();

		await initialize_repository(root);
		await run_git(root, ["config", "--local", key, value]);

		const mutation = await make_mutation(root);
		const failure = await Effect.runPromise(
			mutation.Prepare({ branch: "feature", type: "branch_create" }).pipe(Effect.flip),
		);

		expect(failure.operation).toBe("prepare");
	});

	it("does not invoke executable repository configuration while checking out", async () => {
		const root = await make_root();
		const marker = `${root}.config-invocations.log`;

		roots.push(marker);

		await initialize_repository(root);
		await fs.writeFile(join(root, ".gitattributes"), "tracked.txt filter=adversarial\n");
		await run_git(root, ["add", ".gitattributes"]);
		await run_git(root, ["commit", "--quiet", "--message", "attributes"]);
		await run_git(root, ["switch", "--quiet", "--create", "feature"]);
		await commit_file(root, "tracked.txt", "feature\n", "feature");
		await run_git(root, ["switch", "--quiet", "master"]);
		await configure_executable_repository_settings(root, marker);

		const mutation = await make_mutation(root);
		const failure = await Effect.runPromise(
			mutation.Prepare({ target_branch: "feature", type: "checkout" }).pipe(Effect.flip),
		);

		expect(failure.operation).toBe("prepare");
		await expect(fs.readFile(marker, "utf8")).rejects.toMatchObject({ code: "ENOENT" });
	});

	it("quarantines executable global Git configuration", async () => {
		const root = await make_root();
		const home = join(root, "home");
		const marker = join(root, "global-config-invocations.log");
		const script = join(root, "record-global-config-invocation.mjs");
		const config = join(home, ".gitconfig");
		const command = `${shell_argument(process.execPath)} ${shell_argument(script)} ${shell_argument(marker)}`;
		const keys = ["HOME", "USERPROFILE", "XDG_CONFIG_HOME"] as const;
		const previous = new Map(keys.map((key) => [key, process.env[key]]));

		await fs.mkdir(home);
		await fs.writeFile(
			script,
			[
				'import { appendFileSync } from "node:fs";',
				"",
				"appendFileSync(process.argv[2], `${process.argv[3]}\\n`);",
				'if (process.argv[3] === "fsmonitor") {',
				'\tprocess.stdout.write("token\\0");',
				"} else {",
				"\tprocess.stdin.pipe(process.stdout);",
				"}",
				"",
			].join("\n"),
		);
		await run_git(root, ["config", "--file", config, "core.fsmonitor", `${command} fsmonitor`]);
		await run_git(root, [
			"config",
			"--file",
			config,
			"filter.adversarial.clean",
			`${command} clean`,
		]);
		await run_git(root, [
			"config",
			"--file",
			config,
			"filter.adversarial.smudge",
			`${command} smudge`,
		]);
		await run_git(root, ["config", "--file", config, "filter.adversarial.required", "true"]);
		await initialize_repository(root);
		await fs.writeFile(join(root, ".gitattributes"), "tracked.txt filter=adversarial\n");
		await run_git(root, ["add", ".gitattributes"]);
		await run_git(root, ["commit", "--quiet", "--message", "attributes"]);
		await run_git(root, ["switch", "--quiet", "--create", "feature"]);
		await commit_file(root, "tracked.txt", "feature\n", "feature");
		await run_git(root, ["switch", "--quiet", "master"]);

		for (const key of keys) {
			process.env[key] = home;
		}

		try {
			const mutation = await make_mutation(root);
			const plan = await Effect.runPromise(
				mutation.Prepare({ target_branch: "feature", type: "checkout" }),
			);
			const attempt = await Effect.runPromise(mutation.Execute(plan));
			const outcome = await Effect.runPromise(mutation.Reconcile(plan, attempt));

			expect(outcome.type).toBe("applied");
		} finally {
			for (const key of keys) {
				const value = previous.get(key);

				if (value === undefined) {
					delete process.env[key];
				} else {
					process.env[key] = value;
				}
			}
		}

		await expect(fs.readFile(marker, "utf8")).rejects.toMatchObject({ code: "ENOENT" });
	});

	it("rejects executable worktree-scoped Git configuration before invocation", async () => {
		const root = await make_root();
		const marker = `${root}.worktree-config-invocations.log`;

		roots.push(marker);

		await initialize_repository(root);
		await run_git(root, ["config", "extensions.worktreeConfig", "true"]);
		await configure_executable_repository_settings(root, marker, "--worktree");

		const mutation = await make_mutation(root);
		const failure = await Effect.runPromise(
			mutation.Prepare({ branch: "feature", type: "branch_create" }).pipe(Effect.flip),
		);

		expect(failure.operation).toBe("prepare");
		await expect(fs.readFile(marker, "utf8")).rejects.toMatchObject({ code: "ENOENT" });
	});

	it("rejects a symbolic local branch instead of resolving through it", async () => {
		const root = await make_root();

		await initialize_repository(root);
		await run_git(root, ["symbolic-ref", "refs/heads/alias", "refs/heads/master"]);

		const mutation = await make_mutation(root);
		const failure = await Effect.runPromise(
			mutation.Prepare({ target_branch: "alias", type: "checkout" }).pipe(Effect.flip),
		);

		expect(failure.operation).toBe("prepare");
		expect(await read_git(root, ["symbolic-ref", "--short", "HEAD"])).toBe("master");
	});

	it("does not attach HEAD to a new branch after its source branch moves", async () => {
		const root = await make_root();
		let armed = false;
		let raced = false;

		await initialize_repository(root);
		const source_head = await read_git(root, ["rev-parse", "HEAD"]);
		await run_git(root, ["switch", "--quiet", "--create", "racer"]);
		const racer_head = await commit_file(root, "tracked.txt", "racer\n", "racer");
		await run_git(root, ["switch", "--quiet", "master"]);

		const mutation = await make_hooked_mutation(root, {
			before: async (input) => {
				if (
					armed &&
					!raced &&
					input.args.includes("update-ref") &&
					input.stdin !== undefined
				) {
					raced = true;
					await run_git(root, [
						"update-ref",
						"refs/heads/master",
						racer_head,
						source_head,
					]);
				}
			},
		});
		const plan = await Effect.runPromise(
			mutation.Prepare({ branch: "feature", type: "branch_create" }),
		);

		armed = true;

		const attempt = await Effect.runPromise(mutation.Execute(plan));
		const outcome = await Effect.runPromise(mutation.Reconcile(plan, attempt));
		const head_branch = await run_git(
			root,
			["symbolic-ref", "--quiet", "--short", "HEAD"],
			[0, 1],
		);

		expect(raced).toBe(true);
		expect(outcome.type).not.toBe("applied");
		expect(head_branch.stdout.trim()).not.toBe("feature");
		expect(await read_git(root, ["rev-parse", "refs/heads/master"])).toBe(racer_head);
	});

	it("treats REBASE_HEAD without a live rebase directory as inactive", async () => {
		const root = await make_root();

		await initialize_repository(root);
		const rebase_head_path = await read_git(root, ["rev-parse", "--git-path", "REBASE_HEAD"]);
		const source_head = await read_git(root, ["rev-parse", "HEAD"]);

		await fs.writeFile(join(root, rebase_head_path), `${source_head}\n`);

		const mutation = await make_mutation(root);
		const plan = await Effect.runPromise(
			mutation.Prepare({ branch: "feature", type: "branch_create" }),
		);

		expect(plan.source.state).toBe("none");
		expect((await fs.readFile(join(root, rebase_head_path), "utf8")).trim()).toBe(source_head);
		expect(await read_git(root, ["symbolic-ref", "--short", "HEAD"])).toBe("master");
	});
});

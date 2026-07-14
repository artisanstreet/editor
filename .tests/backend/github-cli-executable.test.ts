import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	GitHubCliExecutable,
	GitHubCliGitExecutable,
	make_node_github_cli_executable_layer,
	make_node_github_cli_git_executable_layer,
} from "../../modules/backend/src/git-provider/github/github-cli-executable";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan gh executable "));

	roots.push(root);

	return root;
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("GitHubCliExecutable", () => {
	it("resolves the first case-insensitive Windows PATH/PATHEXT candidate canonically", async () => {
		const root = await make_root();
		const first_directory = join(root, "first");
		const second_directory = join(root, "second");
		const executable = join(first_directory, "gh.ExE");

		await fs.mkdir(first_directory);
		await fs.mkdir(second_directory);
		await fs.writeFile(executable, "fixture");

		const service = await Effect.runPromise(
			Effect.service(GitHubCliExecutable).pipe(
				Effect.provide(
					make_node_github_cli_executable_layer({
						cwd: root,
						environment: {
							PaTh: `"${first_directory}";${second_directory}`,
							pathext: ".COM;.ExE",
						},
						platform: "win32",
					}),
				),
			),
		);
		const location = await Effect.runPromise(service.Locate);

		expect(Option.isSome(location)).toBe(true);
		if (Option.isNone(location)) {
			return;
		}

		expect(location.value.path.toLowerCase()).toBe(
			(await fs.realpath(join(first_directory, "gh.exe"))).toLowerCase(),
		);
		expect(location.value.path).toMatch(/^[A-Z]:\\/iu);
	});

	it("reports a missing command and pins the first result for the service lifetime", async () => {
		const root = await make_root();
		const first_directory = join(root, "first");
		const second_directory = join(root, "second");
		const first_executable = join(first_directory, "gh.exe");
		const second_executable = join(second_directory, "gh.exe");

		await fs.mkdir(first_directory);
		await fs.mkdir(second_directory);

		const missing_service = await Effect.runPromise(
			Effect.service(GitHubCliExecutable).pipe(
				Effect.provide(
					make_node_github_cli_executable_layer({
						cwd: root,
						environment: { PATH: join(root, "missing"), PATHEXT: ".EXE" },
						platform: "win32",
					}),
				),
			),
		);

		expect(await Effect.runPromise(missing_service.Locate)).toEqual(Option.none());

		const installed_after_start = join(root, "missing", "gh.exe");

		await fs.mkdir(join(root, "missing"));
		await fs.writeFile(installed_after_start, "installed after startup");

		const discovered_after_install = await Effect.runPromise(missing_service.Locate);

		expect(Option.isSome(discovered_after_install)).toBe(true);

		await fs.writeFile(first_executable, "first");
		const service = await Effect.runPromise(
			Effect.service(GitHubCliExecutable).pipe(
				Effect.provide(
					make_node_github_cli_executable_layer({
						cwd: root,
						environment: {
							PATH: `${first_directory};${second_directory}`,
							PATHEXT: ".EXE",
						},
						platform: "win32",
					}),
				),
			),
		);
		const first_location = await Effect.runPromise(service.Locate);

		await fs.rm(first_executable);
		await fs.writeFile(second_executable, "second");

		expect(await Effect.runPromise(service.Locate)).toEqual(first_location);
	});

	it("pins Git independently from the GitHub CLI executable", async () => {
		const root = await make_root();
		const directory = join(root, "tools");
		const git_path = join(directory, "git.exe");

		await fs.mkdir(directory);
		await fs.writeFile(join(directory, "gh.exe"), "github cli");
		await fs.writeFile(git_path, "git");

		const service = await Effect.runPromise(
			Effect.service(GitHubCliGitExecutable).pipe(
				Effect.provide(
					make_node_github_cli_git_executable_layer({
						cwd: root,
						environment: { PATH: directory, PATHEXT: ".EXE" },
						platform: "win32",
					}),
				),
			),
		);
		const location = await Effect.runPromise(service.Locate);

		expect(Option.getOrThrow(location).path.toLowerCase()).toBe(
			(await fs.realpath(git_path)).toLowerCase(),
		);
	});
});

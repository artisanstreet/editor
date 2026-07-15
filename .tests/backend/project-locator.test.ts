import { createHash } from "node:crypto";
import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, normalize, resolve } from "node:path";

import { Effect, Layer, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_node_project_locator_layer,
	ProjectLocator,
	ProjectLocatorError,
} from "../../modules/backend/src/threads/project-locator";
import {
	ProcessRunner,
	ProcessRunnerError,
	type ProcessRunnerInput,
	type ProcessRunnerResult,
} from "../../modules/backend/src/git/process-runner";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan-project-locator-"));

	roots.push(root);

	return root;
}

function normalize_path(path: string) {
	return normalize(resolve(path))
		.replaceAll("\\", "/")
		.replace(/(?<!^[A-Za-z]:)\/$/, "");
}

function process_result(
	stdout: string,
	input: ProcessRunnerInput,
	exit_code = 0,
): ProcessRunnerResult {
	const bytes = new TextEncoder().encode(stdout);
	const limit = input.max_stdout_bytes ?? bytes.byteLength;
	const retained_stdout = bytes.slice(0, limit);

	return {
		exit_code,
		stderr: new Uint8Array(),
		stderr_bytes: 0,
		stderr_truncated: false,
		stdout: retained_stdout,
		stdout_bytes: bytes.byteLength,
		stdout_truncated: retained_stdout.byteLength < bytes.byteLength,
	};
}

function make_locator(
	run: (input: ProcessRunnerInput) => Effect.Effect<ProcessRunnerResult, ProcessRunnerError>,
) {
	const process_runner_layer = Layer.succeed(ProcessRunner, { Run: run, RunProcessTree: run });
	const project_locator_layer = make_node_project_locator_layer().pipe(
		Layer.provide(process_runner_layer),
	);

	return Effect.runPromise(
		Effect.service(ProjectLocator).pipe(Effect.provide(project_locator_layer)),
	);
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("ProjectLocator", () => {
	it("uses the Git worktree root for a file inside a repository", async () => {
		const root = await make_root();
		const nested = join(root, "src", "feature");
		const file = join(nested, "document.ts");

		await fs.mkdir(nested, { recursive: true });
		await fs.writeFile(file, "export {}\n");

		const locator = await make_locator((input) => {
			expect(input).toMatchObject({
				args: ["rev-parse", "--path-format=absolute", "--show-toplevel"],
				command: "git",
				cwd: nested,
			});

			return Effect.succeed(process_result(`${root}\n`, input));
		});
		const project = await Effect.runPromise(locator.Locate(file));
		const root_path = normalize_path(root);

		expect(Option.getOrThrow(project)).toEqual({
			project: {
				display_name: basename(root_path),
				project_id: `project_${createHash("sha256").update(root_path).digest("hex")}`,
				root_path,
			},
			source: "git_root",
		});
	});

	it("uses a directory location directly when discovering its worktree", async () => {
		const root = await make_root();
		const directory = join(root, "packages", "editor");

		await fs.mkdir(directory, { recursive: true });

		const locator = await make_locator((input) => {
			expect(input.cwd).toBe(directory);

			return Effect.succeed(process_result(`${root}\n`, input));
		});
		const project = await Effect.runPromise(locator.Locate(directory));

		expect(Option.getOrThrow(project).project.root_path).toBe(normalize_path(root));
	});

	it("falls back to the normalized directory when Git finds no repository", async () => {
		const directory = await make_root();
		const locator = await make_locator((input) =>
			Effect.succeed(process_result("", input, 128)),
		);
		const project = await Effect.runPromise(locator.Locate(directory));
		const root_path = normalize_path(directory);

		expect(Option.getOrThrow(project)).toMatchObject({
			project: {
				display_name: basename(root_path),
				root_path,
			},
			source: "directory",
		});
	});

	it("normalizes slash styles into one portable identity", async () => {
		const root = await make_root();
		const directory = join(root, "nested");

		await fs.mkdir(directory);

		const forward_slash_locator = await make_locator((input) =>
			Effect.succeed(process_result(`${root.replaceAll("\\", "/")}\n`, input)),
		);
		const backslash_locator = await make_locator((input) =>
			Effect.succeed(process_result(`${root.replaceAll("/", "\\")}\n`, input)),
		);
		const forward_project = Option.getOrThrow(
			await Effect.runPromise(forward_slash_locator.Locate(directory)),
		);
		const backslash_project = Option.getOrThrow(
			await Effect.runPromise(backslash_locator.Locate(directory)),
		);

		expect(forward_project).toEqual(backslash_project);
		expect(forward_project.project.root_path).toBe(normalize_path(root));
	});

	it("models ProcessRunner failures as ProjectLocator discovery failures", async () => {
		const directory = await make_root();
		const failure = new ProcessRunnerError({
			cause: new Error("Git was unavailable"),
			command: "git",
			operation: "spawn",
		});
		const locator = await make_locator(() => Effect.fail(failure));
		const error = await Effect.runPromise(locator.Locate(directory).pipe(Effect.flip));

		expect(error).toBeInstanceOf(ProjectLocatorError);
		expect(error).toMatchObject({ cause: failure, location: directory, operation: "discover" });
	});

	it("resolves a deleted file path from its nearest existing parent", async () => {
		const root = await make_root();
		const source = join(root, "src");
		const deleted_file = join(source, "deleted", "file.ts");

		await fs.mkdir(source);

		const locator = await make_locator((input) => {
			expect(input.cwd).toBe(source);

			return Effect.succeed(process_result(`${root}\n`, input));
		});
		const located = Option.getOrThrow(await Effect.runPromise(locator.Locate(deleted_file)));

		expect(located.project.root_path).toBe(normalize_path(root));
		expect(located.source).toBe("git_root");
	});

	it("rejects an empty project location before spawning Git", async () => {
		const locator = await make_locator(() =>
			Effect.die("Git must not run for an empty project location"),
		);
		const error = await Effect.runPromise(locator.Locate(" ").pipe(Effect.flip));

		expect(error).toBeInstanceOf(ProjectLocatorError);
		expect(error).toMatchObject({ location: " ", operation: "normalize" });
	});
});

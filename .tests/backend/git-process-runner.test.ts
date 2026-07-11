import { Buffer } from "node:buffer";
import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";

import { Effect, Exit } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_node_process_runner_layer } from "../../modules/backend/src/git/node-process-runner";
import { ProcessRunner } from "../../modules/backend/src/git/process-runner";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(`${tmpdir()}/artisan process runner `);

	roots.push(root);

	return root;
}

async function make_runner() {
	return Effect.runPromise(
		Effect.service(ProcessRunner).pipe(Effect.provide(make_node_process_runner_layer())),
	);
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("ProcessRunner", () => {
	it("retains bounded stdout while counting all observed bytes", async () => {
		const root = await make_root();
		const runner = await make_runner();
		const result = await Effect.runPromise(
			runner.Run({
				args: [
					"-e",
					'process.stdout.write("x".repeat(4096)); process.stderr.write("e".repeat(2048))',
				],
				command: process.execPath,
				cwd: root,
				max_stderr_bytes: 16,
				max_stdout_bytes: 32,
			}),
		);

		expect(Buffer.from(result.stdout).toString("utf8")).toBe("x".repeat(32));
		expect(result.stdout.byteLength).toBe(32);
		expect(result.stdout_bytes).toBe(4096);
		expect(result.stdout_truncated).toBe(true);
		expect(Buffer.from(result.stderr).toString("utf8")).toBe("e".repeat(16));
		expect(result.stderr_bytes).toBe(2048);
		expect(result.stderr_truncated).toBe(true);
	});

	it("reports spawn failures as process errors", async () => {
		const root = await make_root();
		const runner = await make_runner();
		const error = await Effect.runPromise(
			runner
				.Run({ args: [], command: "artisan-command-that-does-not-exist", cwd: root })
				.pipe(Effect.flip),
		);

		expect(error._tag).toBe("ProcessRunnerError");
		expect(error.operation).toBe("spawn");
	});

	it("applies isolated environment overrides without shell interpolation", async () => {
		const root = await make_root();
		const runner = await make_runner();
		const result = await Effect.runPromise(
			runner.Run({
				args: ["-e", 'process.stdout.write(process.env.ARTISAN_PROBE ?? "missing")'],
				command: process.execPath,
				cwd: root,
				environment: { ARTISAN_PROBE: "present with spaces" },
			}),
		);

		expect(Buffer.from(result.stdout).toString("utf8")).toBe("present with spaces");
	});

	it("interrupts and cleans up a long-running child", async () => {
		const root = await make_root();
		const runner = await make_runner();
		const started_at = Date.now();
		const result = await Effect.runPromiseExit(
			runner
				.Run({
					args: ["-e", "setTimeout(() => {}, 10_000)"],
					command: process.execPath,
					cwd: root,
				})
				.pipe(Effect.timeout("100 millis")),
		);

		expect(Exit.isFailure(result)).toBe(true);
		expect(Date.now() - started_at).toBeLessThan(2_000);
	});
});

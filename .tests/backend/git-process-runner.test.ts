import { Buffer } from "node:buffer";
import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";

import { Effect, Fiber } from "effect";
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

async function wait_for_path(path: string) {
	const expires_at = Date.now() + 2_000;

	while (Date.now() < expires_at) {
		try {
			await fs.access(path);

			return;
		} catch {
			await new Promise((resolve) => setTimeout(resolve, 10));
		}
	}

	throw new Error(`Timed out waiting for ${path}`);
}

async function wait_for_process_exit(process_id: number) {
	const expires_at = Date.now() + 5_000;

	while (Date.now() < expires_at) {
		try {
			process.kill(process_id, 0);
		} catch {
			return;
		}

		await new Promise((resolve) => setTimeout(resolve, 10));
	}

	throw new Error(`Timed out waiting for process ${process_id} to exit`);
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

	it("can replace the inherited environment for capability-confined processes", async () => {
		const root = await make_root();
		const runner = await make_runner();
		const inherited_key = "ARTISAN_INHERITED_ONLY";
		const previous = process.env[inherited_key];

		process.env[inherited_key] = "parent";

		try {
			const result = await Effect.runPromise(
				runner.Run({
					args: [
						"-e",
						'process.stdout.write(`${process.env.ARTISAN_PROBE ?? "missing"}|${process.env.ARTISAN_INHERITED_ONLY ?? "missing"}`)',
					],
					command: process.execPath,
					cwd: root,
					environment: { ARTISAN_PROBE: "isolated" },
					environment_mode: "replace",
				}),
			);

			expect(Buffer.from(result.stdout).toString("utf8")).toBe("isolated|missing");
		} finally {
			if (previous === undefined) {
				delete process.env[inherited_key];
			} else {
				process.env[inherited_key] = previous;
			}
		}
	});

	it("delivers an owned binary stdin copy and ends the child stream", async () => {
		const root = await make_root();
		const runner = await make_runner();
		const stdin = new Uint8Array([0, 255, 16, 0, 127, 64]);
		const result_promise = Effect.runPromise(
			runner.Run({
				args: [
					"-e",
					'const chunks = []; process.stdin.on("data", (chunk) => chunks.push(chunk)); process.stdin.on("end", () => process.stdout.write(Buffer.concat(chunks).toString("base64")))',
				],
				command: process.execPath,
				cwd: root,
				stdin,
			}),
		);

		stdin.fill(1);

		const result = await result_promise;

		expect(Buffer.from(result.stdout).toString("utf8")).toBe("AP8QAH9A");
	});

	it("rejects oversized stdin before spawning the child", async () => {
		const root = await make_root();
		const marker_path = `${root}/spawned.txt`;
		const runner = await Effect.runPromise(
			Effect.service(ProcessRunner).pipe(
				Effect.provide(make_node_process_runner_layer({ max_stdin_bytes: 3 })),
			),
		);
		const error = await Effect.runPromise(
			runner
				.Run({
					args: [
						"-e",
						'require("node:fs").writeFileSync(process.argv[1], "spawned")',
						marker_path,
					],
					command: process.execPath,
					cwd: root,
					stdin: new Uint8Array([1, 2, 3, 4]),
				})
				.pipe(Effect.flip),
		);

		expect(error.operation).toBe("configuration");
		await expect(fs.access(marker_path)).rejects.toThrow();
	});

	it("reports an early stdin close as a typed failure without an unhandled error", async () => {
		const root = await make_root();
		const runner = await Effect.runPromise(
			Effect.service(ProcessRunner).pipe(
				Effect.provide(
					make_node_process_runner_layer({
						kill_timeout_ms: 50,
						max_stdin_bytes: 16 * 1024 * 1024,
					}),
				),
			),
		);
		const error = await Effect.runPromise(
			runner
				.Run({
					args: [
						"-e",
						'process.stdin.once("data", () => { process.stdin.destroy(); setTimeout(() => process.exit(0), 25) })',
					],
					command: process.execPath,
					cwd: root,
					stdin: new Uint8Array(8 * 1024 * 1024),
				})
				.pipe(Effect.flip),
		);

		expect(error._tag).toBe("ProcessRunnerError");
		expect(error.operation).toBe("stdin");
		expect(error.cause).toBeInstanceOf(Error);
	});

	it("interrupts and terminates a long-running child with pending stdin", async () => {
		const root = await make_root();
		const marker_path = `${root}/child-process-id`;
		const runner = await Effect.runPromise(
			Effect.service(ProcessRunner).pipe(
				Effect.provide(make_node_process_runner_layer({ kill_timeout_ms: 50 })),
			),
		);
		const process_id = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const process_fiber = yield* runner
						.Run({
							args: [
								"-e",
								'require("node:fs").writeFileSync(process.argv[1], String(process.pid)); setTimeout(() => {}, 10_000)',
								marker_path,
							],
							command: process.execPath,
							cwd: root,
							stdin: new Uint8Array(1024 * 1024),
						})
						.pipe(Effect.forkScoped);

					yield* Effect.promise(() => wait_for_path(marker_path));
					const process_id = yield* Effect.promise(() =>
						fs.readFile(marker_path, "utf8").then(Number),
					);

					yield* Fiber.interrupt(process_fiber);

					return process_id;
				}),
			),
		);

		expect(process_id).toBeGreaterThan(0);
		await wait_for_process_exit(process_id);
	});
});

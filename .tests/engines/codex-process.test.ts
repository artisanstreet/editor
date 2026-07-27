import { Buffer } from "node:buffer";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";
import { Effect, Fiber } from "effect";

import {
	CodexProcessFactory,
	CodexProcessFactoryLive,
	type CodexProcessHandle,
} from "@artisan/engines";

const fixture_path = fileURLToPath(new URL("./fixtures/fake-child.ts", import.meta.url));
const cmd_fixture_path = fileURLToPath(new URL("./fixtures/fake-codex.cmd", import.meta.url));
const spawning_children_fixture_path = fileURLToPath(
	new URL("./fixtures/fake-spawning-children.ts", import.meta.url),
);
const encoder = new TextEncoder();
const decoder = new TextDecoder();

function is_process_alive(pid: number) {
	try {
		process.kill(pid, 0);

		return true;
	} catch {
		return false;
	}
}

function make_scenario(
	chunks: ReadonlyArray<unknown>,
	exit?: { readonly at_ms: number; readonly code: number },
) {
	return encoder.encode(`${JSON.stringify({ chunks, ...(exit ? { exit } : {}) })}\n`);
}

function read_stdout(handle: CodexProcessHandle) {
	return Effect.tryPromise({
		try: async () => {
			const chunks: Array<Uint8Array> = [];

			for await (const chunk of handle.Stdout) {
				chunks.push(chunk);
			}

			return decoder.decode(Buffer.concat(chunks));
		},
		catch: (cause) => cause,
	});
}

function spawn_fixture() {
	return Effect.gen(function* () {
		const factory = yield* CodexProcessFactory;

		return yield* factory.Spawn({
			args: [fixture_path],
			command: process.execPath,
		});
	});
}

describe("Codex process factory", () => {
	it("closes an already exited child without active-process discovery", async () => {
		const elapsed_ms = await Effect.runPromise(
			Effect.gen(function* () {
				const handle = yield* spawn_fixture();

				yield* handle.Write(make_scenario([], { at_ms: 0, code: 0 }));
				yield* handle.Exit;

				const started_at = performance.now();

				yield* handle.Close;

				return performance.now() - started_at;
			}).pipe(Effect.scoped, Effect.provide(CodexProcessFactoryLive)),
		);

		expect(elapsed_ms).toBeLessThan(750);
	});

	it("starts a real fixture, transfers exact chunks, and observes exit", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const handle = yield* spawn_fixture();
				const output_fiber = yield* Effect.forkChild(read_stdout(handle));

				yield* handle.Write(
					make_scenario(
						[
							{ at_ms: 0, chunk_base64: "aGVs" },
							{ at_ms: 1, chunk_base64: "bG8K" },
						],
						{ at_ms: 5, code: 7 },
					),
				);

				return {
					exit: yield* handle.Exit,
					output: yield* Fiber.join(output_fiber),
				};
			}).pipe(Effect.scoped, Effect.provide(CodexProcessFactoryLive)),
		);

		expect(result.output).toBe("hello\n");
		expect(result.exit).toMatchObject({ code: 7, signal: null });
	});

	it("cancels an active child process through its handle", async () => {
		const exit = await Effect.runPromise(
			Effect.gen(function* () {
				const handle = yield* spawn_fixture();

				yield* handle.Write(make_scenario([]));
				yield* handle.Kill();

				return yield* handle.Exit;
			}).pipe(Effect.scoped, Effect.provide(CodexProcessFactoryLive)),
		);

		if (process.platform === "win32") {
			expect(exit).toEqual({ code: 1, signal: null });
		} else {
			expect(exit.code === 143 || exit.signal === "SIGTERM").toBe(true);
		}
	});

	it("cleans up a live child process when its handle closes", async () => {
		const exit = await Effect.runPromise(
			Effect.gen(function* () {
				const handle = yield* spawn_fixture();

				yield* handle.Write(make_scenario([]));
				yield* handle.Close;

				return yield* handle.Exit;
			}).pipe(Effect.scoped, Effect.provide(CodexProcessFactoryLive)),
		);

		expect(exit).not.toEqual({ code: 0, signal: null });
	});

	it("cleans up the process tree when its owning scope is interrupted", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-engine-interruption-"));
		const pid_path = join(directory, "children.pid");

		try {
			const child_pids = await Effect.runPromise(
				Effect.gen(function* () {
					const owner = yield* Effect.gen(function* () {
						const factory = yield* CodexProcessFactory;

						yield* factory.Spawn({
							args: [spawning_children_fixture_path],
							command: process.execPath,
							env: { ...process.env, FAKE_CHILD_PID_FILE: pid_path },
						});
						yield* Effect.never;
					}).pipe(
						Effect.scoped,
						Effect.provide(CodexProcessFactoryLive),
						Effect.forkChild,
					);
					const pids = yield* Effect.promise(async () => {
						for (let attempt = 0; attempt < 100; attempt += 1) {
							try {
								const observed = (await readFile(pid_path, "utf8"))
									.trim()
									.split("\n")
									.map(Number);

								if (observed.length >= 2) {
									return observed;
								}
							} catch {
								/** The fixture has not written its first child yet. */
							}

							await new Promise((resolve) => setTimeout(resolve, 10));
						}

						throw new Error("Timed out waiting for interrupted process tree");
					});

					yield* Fiber.interrupt(owner);

					return pids;
				}),
			);

			await expect
				.poll(() => child_pids.every((pid) => !is_process_alive(pid)), { timeout: 5_000 })
				.toBe(true);
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});

	it("cleans up a grandchild when stdin close makes the parent exit", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-engine-"));
		const pid_path = join(directory, "grandchild.pid");

		try {
			await Effect.runPromise(
				Effect.gen(function* () {
					const factory = yield* CodexProcessFactory;
					const handle = yield* factory.Spawn({
						args: [
							fixture_path.replace("fake-child.ts", "fake-stdin-exit-grandchild.ts"),
						],
						command: process.execPath,
						env: { ...process.env, FAKE_GRANDCHILD_PID_FILE: pid_path },
					});

					const grandchild_pid = yield* Effect.promise(async () => {
						for (let attempt = 0; attempt < 50; attempt += 1) {
							try {
								return Number(await readFile(pid_path, "utf8"));
							} catch {
								await new Promise((resolve) => setTimeout(resolve, 10));
							}
						}
						throw new Error("Timed out waiting for grandchild PID");
					});

					yield* handle.Close;
					yield* handle.Exit;

					yield* Effect.promise(async () => {
						for (let attempt = 0; attempt < 50; attempt += 1) {
							try {
								process.kill(grandchild_pid, 0);
								await new Promise((resolve) => setTimeout(resolve, 10));
							} catch {
								return;
							}
						}
						throw new Error("Timed out waiting for grandchild exit");
					});
				}).pipe(Effect.scoped, Effect.provide(CodexProcessFactoryLive)),
			);
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});

	it("fences child creation while closing the process tree", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-engine-spawn-race-"));
		const pid_path = join(directory, "children.pid");

		try {
			const child_pids = await Effect.runPromise(
				Effect.gen(function* () {
					const factory = yield* CodexProcessFactory;
					const handle = yield* factory.Spawn({
						args: [spawning_children_fixture_path],
						command: process.execPath,
						env: { ...process.env, FAKE_CHILD_PID_FILE: pid_path },
					});

					yield* Effect.promise(async () => {
						for (let attempt = 0; attempt < 100; attempt += 1) {
							try {
								const pids = (await readFile(pid_path, "utf8")).trim().split("\n");

								if (pids.length >= 5) {
									return;
								}
							} catch {
								/** The fixture has not written its first child yet. */
							}

							await new Promise((resolve) => setTimeout(resolve, 10));
						}

						throw new Error("Timed out waiting for the child-spawn race fixture");
					});

					yield* handle.Close;
					yield* handle.Exit;

					return (yield* Effect.promise(() => readFile(pid_path, "utf8")))
						.trim()
						.split("\n")
						.map(Number);
				}).pipe(Effect.scoped, Effect.provide(CodexProcessFactoryLive)),
			);

			await expect
				.poll(() => child_pids.every((pid) => !is_process_alive(pid)), { timeout: 5_000 })
				.toBe(true);
		} finally {
			try {
				const remaining_pids = (await readFile(pid_path, "utf8"))
					.trim()
					.split("\n")
					.map(Number);

				for (const pid of remaining_pids) {
					try {
						process.kill(pid, "SIGKILL");
					} catch {
						/** The process was already cleaned up by the handle. */
					}
				}
			} catch {
				/** The fixture failed before creating a child. */
			}

			await rm(directory, { force: true, recursive: true });
		}
	});

	it.skipIf(process.platform !== "win32")(
		"isolates concurrent engine trees in private Windows jobs",
		async () => {
			const directory = await mkdtemp(join(tmpdir(), "artisan-engine-isolation-"));
			const first_pid_path = join(directory, "first.pid");
			const second_pid_path = join(directory, "second.pid");
			let second_handle: CodexProcessHandle | undefined;

			try {
				const result = await Effect.runPromise(
					Effect.gen(function* () {
						const factory = yield* CodexProcessFactory;
						const first_handle = yield* factory.Spawn({
							args: [spawning_children_fixture_path],
							command: process.execPath,
							env: { ...process.env, FAKE_CHILD_PID_FILE: first_pid_path },
						});
						second_handle = yield* factory.Spawn({
							args: [spawning_children_fixture_path],
							command: process.execPath,
							env: { ...process.env, FAKE_CHILD_PID_FILE: second_pid_path },
						});

						const pids = yield* Effect.promise(async () => {
							for (let attempt = 0; attempt < 100; attempt += 1) {
								try {
									const first = (await readFile(first_pid_path, "utf8"))
										.trim()
										.split("\n")
										.map(Number);
									const second = (await readFile(second_pid_path, "utf8"))
										.trim()
										.split("\n")
										.map(Number);

									if (first.length >= 2 && second.length >= 2) {
										return { first, second };
									}
								} catch {
									/** Both fixtures are still starting. */
								}

								await new Promise((resolve) => setTimeout(resolve, 10));
							}

							throw new Error("Timed out waiting for isolated process trees");
						});

						yield* first_handle.Close;
						yield* first_handle.Exit;

						return {
							...pids,
							second_was_alive: pids.second.some(is_process_alive),
						};
					}).pipe(Effect.scoped, Effect.provide(CodexProcessFactoryLive)),
				);

				await expect
					.poll(() => result.first.every((pid) => !is_process_alive(pid)), {
						timeout: 2_000,
					})
					.toBe(true);
				expect(result.second_was_alive).toBe(true);
			} finally {
				if (second_handle) {
					await Effect.runPromise(second_handle.Close);
				}

				for (const pid_path of [first_pid_path, second_pid_path]) {
					try {
						const pids = (await readFile(pid_path, "utf8"))
							.trim()
							.split("\n")
							.map(Number);

						for (const pid of pids) {
							try {
								process.kill(pid, "SIGKILL");
							} catch {
								/** The private job already released this process. */
							}
						}
					} catch {
						/** The fixture did not produce a PID file. */
					}
				}

				await rm(directory, { force: true, recursive: true });
			}
		},
	);

	it.skipIf(process.platform !== "win32")(
		"passes metacharacter-bearing argv and stdin through a PATH-style cmd launcher exactly",
		async () => {
			const directory = await mkdtemp(join(tmpdir(), "artisan-cmd-spawn-"));
			const invocation_path = join(directory, "invocation.jsonl");
			const stdin_path = join(directory, "stdin.txt");
			const model = "model & echo argv-injected";
			const prompt = "prompt & echo stdin-injected";

			try {
				await Effect.runPromise(
					Effect.gen(function* () {
						const factory = yield* CodexProcessFactory;
						const handle = yield* factory.Spawn({
							args: ["exec", "--model", model, "-"],
							command: cmd_fixture_path,
							env: {
								...process.env,
								ARTISAN_NODE_EXECUTABLE: process.execPath,
								FAKE_CODEX_EXEC_INVOCATION_FILE: invocation_path,
								FAKE_CODEX_EXEC_STDIN_FILE: stdin_path,
							},
						});

						yield* handle.Write(encoder.encode(prompt));
						yield* handle.EndInput;
						yield* handle.Exit;
					}).pipe(Effect.scoped, Effect.provide(CodexProcessFactoryLive)),
				);

				expect(JSON.parse(await readFile(invocation_path, "utf8"))).toEqual([
					"exec",
					"--model",
					model,
					"-",
				]);
				expect(await readFile(stdin_path, "utf8")).toBe(prompt);
			} finally {
				await rm(directory, { force: true, recursive: true });
			}
		},
	);
});

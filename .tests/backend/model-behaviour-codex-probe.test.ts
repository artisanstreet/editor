import { Buffer } from "node:buffer";
import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";

import { Cause, Deferred, Effect, Exit, Fiber, Layer } from "effect";
import { TestClock } from "effect/testing";
import { afterEach, describe, expect, it } from "vitest";

import {
	CodexModelBehaviourProbe,
	make_codex_model_behaviour_executable_layer,
	make_codex_model_behaviour_probe_layer,
} from "../../modules/backend/src/model-behaviour/codex-probe";
import {
	ProcessRunner,
	ProcessRunnerError,
	type ProcessRunnerInput,
	type ProcessRunnerResult,
} from "../../modules/backend/src/git/process-runner";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(`${tmpdir()}/artisan codex probe test `);

	roots.push(root);

	return root;
}

function process_result(report: unknown, exit_code = 0): ProcessRunnerResult {
	const stdout = Buffer.from(JSON.stringify(report));

	return {
		exit_code,
		stderr: Buffer.from(""),
		stderr_bytes: 0,
		stderr_truncated: false,
		stdout,
		stdout_bytes: stdout.byteLength,
		stdout_truncated: false,
	};
}

function report(status: string) {
	return {
		checks: { "config.load": { status } },
		codexVersion: "0.142.5",
	};
}

function make_probe(
	root: string,
	run: (input: ProcessRunnerInput) => Effect.Effect<ProcessRunnerResult, ProcessRunnerError>,
) {
	return Effect.service(CodexModelBehaviourProbe).pipe(
		Effect.provide(
			make_codex_model_behaviour_probe_layer({
				command: "codex-test",
				cwd: root,
				temporary_directory: root,
			}).pipe(Layer.provide(Layer.succeed(ProcessRunner, { Run: run }))),
		),
	);
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("CodexModelBehaviourProbe", () => {
	it("recognizes a mapping only when valid and invalid owned values diverge", async () => {
		const root = await make_root();
		const homes: Array<string> = [];
		const probe = await Effect.runPromise(
			make_probe(root, (input) =>
				Effect.tryPromise(async () => {
					const home = input.environment?.CODEX_HOME ?? "";
					const content = await fs.readFile(`${home}/config.toml`, "utf8");

					homes.push(home);

					return process_result(
						report(content.includes("artisan-invalid-probe") ? "fail" : "ok"),
					);
				}).pipe(
					Effect.mapError(
						(cause) =>
							new ProcessRunnerError({
								cause,
								command: input.command,
								operation: "spawn",
							}),
					),
				),
			),
		);
		const result = await Effect.runPromise(probe.Probe);

		expect(result).toEqual({
			installed_version: "0.142.5",
			mapping_available: true,
			type: "available",
		});
		expect(new Set(homes).size).toBe(2);
		expect(homes[0]).toBeDefined();
		await expect(fs.access(homes[0]!)).rejects.toBeDefined();
	});

	it("reports an installed CLI without claiming an unrecognized mapping", async () => {
		const root = await make_root();
		const probe = await Effect.runPromise(
			make_probe(root, () => Effect.succeed(process_result(report("ok")))),
		);

		await expect(Effect.runPromise(probe.Probe)).resolves.toEqual({
			installed_version: "0.142.5",
			mapping_available: false,
			type: "available",
		});
	});

	it("makes process and malformed-output failures explicitly unavailable", async () => {
		const root = await make_root();
		const process_failure = new ProcessRunnerError({
			cause: new Error("missing"),
			command: "codex-test",
			operation: "spawn",
		});
		const failed_probe = await Effect.runPromise(
			make_probe(root, () => Effect.fail(process_failure)),
		);
		const malformed_probe = await Effect.runPromise(
			make_probe(root, () => Effect.succeed(process_result("not-a-report"))),
		);

		await expect(Effect.runPromise(failed_probe.Probe)).resolves.toEqual({
			reason: "process_failed",
			type: "unavailable",
		});
		await expect(Effect.runPromise(malformed_probe.Probe)).resolves.toEqual({
			reason: "invalid_output",
			type: "unavailable",
		});
	});

	it("fails closed when Codex exits non-zero despite emitting a valid report", async () => {
		const root = await make_root();
		const probe = await Effect.runPromise(
			make_probe(root, () => Effect.succeed(process_result(report("ok"), 1))),
		);

		await expect(Effect.runPromise(probe.Probe)).resolves.toEqual({
			reason: "process_failed",
			type: "unavailable",
		});
	});

	it("resolves the managed executable once for concurrent isolated probe spawns", async () => {
		const root = await make_root();
		const commands: Array<string> = [];
		let generation = 0;
		const executable = make_codex_model_behaviour_executable_layer(
			Effect.sync(() => `C:/Artisan/toolchain/codex/${String(++generation)}/codex.exe`),
		);
		const probe_layer = make_codex_model_behaviour_probe_layer({
			cwd: root,
			executable,
			temporary_directory: root,
		}).pipe(
			Layer.provide(
				Layer.succeed(ProcessRunner, {
					Run: (input) =>
						Effect.sync(() => {
							commands.push(input.command);
							return process_result(report("ok"));
						}),
				}),
			),
		);
		const probe = await Effect.runPromise(
			Effect.service(CodexModelBehaviourProbe).pipe(Effect.provide(probe_layer)),
		);

		await Effect.runPromise(probe.Probe);

		expect(commands).toEqual([
			"C:/Artisan/toolchain/codex/1/codex.exe",
			"C:/Artisan/toolchain/codex/1/codex.exe",
		]);
	});

	it("runs both isolated observations concurrently and fails closed at the deadline", async () => {
		await Effect.runPromise(
			Effect.gen(function* () {
				const entered = yield* Deferred.make<void>();
				const both_entered = yield* Deferred.make<void>();
				const release = yield* Deferred.make<void>();
				const root = yield* Effect.promise(make_root);
				let active = 0;
				let peak = 0;
				let resolutions = 0;
				const probe_layer = make_codex_model_behaviour_probe_layer({
					cwd: root,
					executable: make_codex_model_behaviour_executable_layer(
						Effect.sync(() => {
							resolutions += 1;
							return "codex-test";
						}),
					),
					temporary_directory: root,
					timeout: "1 second",
				}).pipe(
					Layer.provide(
						Layer.succeed(ProcessRunner, {
							Run: () =>
								Effect.acquireUseRelease(
									Effect.gen(function* () {
										active += 1;
										peak = Math.max(peak, active);
										if (active === 2) {
											yield* Deferred.succeed(both_entered, undefined);
										}
									}),
									() =>
										Deferred.succeed(entered, undefined).pipe(
											Effect.andThen(Deferred.await(release)),
											Effect.as(process_result(report("ok"))),
										),
									() => Effect.sync(() => void (active -= 1)),
								),
						}),
					),
				);
				const probe = yield* Effect.service(CodexModelBehaviourProbe).pipe(
					Effect.provide(probe_layer),
				);
				const running = yield* probe.Probe.pipe(
					Effect.forkChild({ startImmediately: true }),
				);

				yield* Deferred.await(entered);
				yield* Deferred.await(both_entered);
				expect(peak).toBe(2);
				expect(resolutions).toBe(1);
				expect(
					(yield* Effect.promise(() => fs.readdir(root))).filter((name) =>
						name.startsWith("artisan-codex-model-behaviour-"),
					),
				).toHaveLength(2);
				yield* TestClock.adjust("1 second");
				expect(yield* Fiber.join(running)).toEqual({
					reason: "process_failed",
					type: "unavailable",
				});
				expect(active).toBe(0);
				expect(
					(yield* Effect.promise(() => fs.readdir(root))).filter((name) =>
						name.startsWith("artisan-codex-model-behaviour-"),
					),
				).toEqual([]);
				return yield* Effect.void;
			}).pipe(Effect.provide(TestClock.layer())),
		);
	});

	it("releases both process resources and temporary directories on explicit interruption", async () => {
		const root = await make_root();
		const entered = await Effect.runPromise(Deferred.make<void>());
		const both_entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		let active = 0;
		let released = 0;
		const probe = await Effect.runPromise(
			Effect.service(CodexModelBehaviourProbe).pipe(
				Effect.provide(
					make_codex_model_behaviour_probe_layer({
						command: "codex-test",
						cwd: root,
						temporary_directory: root,
					}).pipe(
						Layer.provide(
							Layer.succeed(ProcessRunner, {
								Run: () =>
									Effect.acquireUseRelease(
										Effect.gen(function* () {
											active += 1;
											if (active === 2) {
												yield* Deferred.succeed(both_entered, undefined);
											}
										}),
										() =>
											Deferred.succeed(entered, undefined).pipe(
												Effect.andThen(Deferred.await(release)),
												Effect.as(process_result(report("ok"))),
											),
										() =>
											Effect.sync(() => {
												active -= 1;
												released += 1;
											}),
									),
							}),
						),
					),
				),
			),
		);
		const running = Effect.runFork(probe.Probe);
		await Effect.runPromise(Deferred.await(entered));
		await Effect.runPromise(Deferred.await(both_entered));
		expect(
			(await fs.readdir(root)).filter((name) =>
				name.startsWith("artisan-codex-model-behaviour-"),
			),
		).toHaveLength(2);

		await Effect.runPromise(Fiber.interrupt(running));
		const exit = await Effect.runPromise(Fiber.await(running));

		expect(Exit.isFailure(exit) && Cause.hasInterruptsOnly(exit.cause)).toBe(true);
		expect(active).toBe(0);
		expect(released).toBe(2);
		expect(
			(await fs.readdir(root)).filter((name) =>
				name.startsWith("artisan-codex-model-behaviour-"),
			),
		).toEqual([]);
	});
});

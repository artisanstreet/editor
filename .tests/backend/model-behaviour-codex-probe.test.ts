import { Buffer } from "node:buffer";
import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";

import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	CodexModelBehaviourProbe,
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
			}).pipe(Layer.provide(Layer.succeed(ProcessRunner, { Run: run, RunProcessTree: run }))),
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
		expect(new Set(homes).size).toBe(1);
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
});

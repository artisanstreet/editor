import { fileURLToPath } from "node:url";

import { describe, expect, it, afterEach } from "vitest";
import { Effect, Layer } from "effect";

import { EngineProcessFactoryLive } from "../../modules/engines/src/process/process";
import {
	ClaudeCliEngine,
	make_claude_cli_engine_layer,
} from "../../modules/engines/src/claude/cli-engine";

const executable = fileURLToPath(new URL("./fixtures/fake-claude.ts", import.meta.url));
const original = process.env.FAKE_CLAUDE_SCENARIO;

afterEach(() => {
	if (original === undefined) delete process.env.FAKE_CLAUDE_SCENARIO;
	else process.env.FAKE_CLAUDE_SCENARIO = original;
});

function engine(options: Record<string, unknown> = {}) {
	return Effect.runPromise(
		ClaudeCliEngine.pipe(
			Effect.provide(
				make_claude_cli_engine_layer({
					executable: process.execPath,
					executable_args: [executable],
					auth_retry_attempts: 0,
					...options,
				}).pipe(Layer.provide(EngineProcessFactoryLive)),
			),
		),
	);
}

describe("Claude readiness probe", () => {
	it("reports authenticated, unauthenticated, and unsupported auth status truthfully", async () => {
		const ready = await Effect.runPromise((await engine()).Probe({}));
		expect(ready.authentication.state).toBe("authenticated");
		process.env.FAKE_CLAUDE_SCENARIO = "auth-unauth";
		const unauthenticated = await Effect.runPromise((await engine()).Probe({}));
		expect(unauthenticated.authentication.state).toBe("unauthenticated");
		expect(unauthenticated.ready).toBe(false);
		process.env.FAKE_CLAUDE_SCENARIO = "auth-unsupported";
		const unsupported = await Effect.runPromise((await engine()).Probe({}));
		expect(unsupported.authentication.state).toBe("unknown");
		expect(unsupported.ready).toBe(false);
	});

	it("does not misreport a signed-out account as a usage lookup failure", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "auth-unauth";
		const claude = await engine();

		await expect(Effect.runPromise(claude.Usage!)).resolves.toEqual({
			authentication: { state: "unauthenticated" },
			windows: [],
		});
	});

	/**
	 * A usage read is gated on auth and nothing else. Running the whole probe for
	 * it charged a second spawn on `--version`, whose only output is a version
	 * string this path never reads, and gave that spawn its own timeout budget to
	 * fail inside — on a deadline every engine refreshes against concurrently.
	 * A broken `--version` must therefore not be able to break a usage read.
	 */
	it("reads usage without spawning the version probe", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "version-nonzero";
		const claude = await engine();

		expect((await Effect.runPromise(Effect.exit(claude.Probe({}))))._tag).toBe("Failure");
		const usage = await Effect.runPromise(claude.Usage!);

		expect(usage.authentication).toEqual({ state: "authenticated" });
		expect(usage.windows.map((window) => window.id)).toEqual(["five_hour", "seven_day"]);
	});

	it("fails missing binaries, nonzero version, bounds, and typed timeouts", async () => {
		const missing = await Effect.runPromise(
			Effect.exit((await engine({ executable: "missing-claude-binary" })).Probe({})),
		);
		expect(missing._tag).toBe("Failure");
		process.env.FAKE_CLAUDE_SCENARIO = "version-nonzero";
		expect((await Effect.runPromise(Effect.exit((await engine()).Probe({}))))._tag).toBe(
			"Failure",
		);
		process.env.FAKE_CLAUDE_SCENARIO = "version-bounds";
		expect(
			(
				await Effect.runPromise(
					Effect.exit((await engine({ max_stdout_bytes: 128 })).Probe({})),
				)
			)._tag,
		).toBe("Failure");
		process.env.FAKE_CLAUDE_SCENARIO = "version-timeout";
		const timeout = await Effect.runPromise(
			Effect.exit((await engine({ version_timeout_ms: 25 })).Probe({})),
		);
		expect(timeout._tag).toBe("Failure");
		expect(JSON.stringify(timeout)).toContain("EngineProbeTimeoutError");
		process.env.FAKE_CLAUDE_SCENARIO = "auth-timeout";
		const auth_timeout = await Effect.runPromise(
			Effect.exit((await engine({ auth_timeout_ms: 25 })).Probe({})),
		);
		expect(auth_timeout._tag).toBe("Failure");
	});

	it("does not open an unauthenticated run", async () => {
		process.env.FAKE_CLAUDE_SCENARIO = "auth-unauth";
		const result = await Effect.runPromise(
			Effect.exit(
				Effect.scoped(
					(await engine()).Open({
						_tag: "start",
						artisan_run_id: "unauth",
						initial_text: "no",
						working_directory: process.cwd(),
					}),
				),
			),
		);
		expect(result._tag).toBe("Failure");
	});
});

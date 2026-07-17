import { tmpdir } from "node:os";

import { describe, expect, it } from "vitest";
import { Cause, Effect, Exit, Fiber, Stream } from "effect";

import {
	NodePtyTerminalDriverLive,
	TerminalDriver,
	make_node_pty_terminal_driver_layer,
} from "@artisan/backend";

function error_from(exit: Exit.Exit<unknown, unknown>) {
	return Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;
}

function open_node_terminal(script: string) {
	return Effect.gen(function* () {
		const driver = yield* TerminalDriver;

		return yield* driver.Open({
			args: ["-e", script],
			cols: 100,
			cwd: tmpdir(),
			executable: process.execPath,
			rows: 30,
		});
	});
}

function is_process_alive(pid: number) {
	try {
		process.kill(pid, 0);

		return true;
	} catch {
		return false;
	}
}

async function wait_for_process_exit(pid: number) {
	for (let attempt = 0; attempt < 40; attempt += 1) {
		if (!is_process_alive(pid)) {
			return;
		}

		await new Promise<void>((resolve) => setTimeout(resolve, 25));
	}
}

describe("TerminalDriver node-pty adapter", () => {
	it("owns interactive PTY output, input, resize, and natural exit", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const terminal = yield* open_node_terminal(
						`process.stdout.write("READY\\n"); process.stdin.once("data", data => { process.stdout.write("ECHO:" + String(data).trim() + "\\n"); process.exit(7); });`,
					);
					const output_fiber = yield* terminal.Output.pipe(
						Stream.runCollect,
						Effect.forkChild,
					);

					yield* terminal.Resize(120, 40);
					yield* terminal.Write(new TextEncoder().encode("ping\r"));

					const exit = yield* terminal.Exit;
					const output_completed_before_exit = output_fiber.pollUnsafe() !== undefined;
					const chunks = yield* Fiber.join(output_fiber);
					const output = new TextDecoder().decode(
						Uint8Array.from([...chunks].flatMap((chunk) => [...chunk])),
					);

					return { exit, output, output_completed_before_exit, pid: terminal.pid };
				}),
			).pipe(Effect.provide(NodePtyTerminalDriverLive)),
		);

		expect(result.pid).toBeGreaterThan(0);
		expect(result.output).toContain("READY");
		expect(result.output).toContain("ECHO:ping");
		expect(result.output_completed_before_exit).toBe(true);
		expect(result.exit).toMatchObject({ exit_code: 7, reason: "exited" });
	});

	it("closes scoped terminals idempotently and rejects invalid dimensions", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const terminal = yield* open_node_terminal(`setInterval(() => {}, 1000);`);
					const output_fiber = yield* terminal.Output.pipe(
						Stream.runDrain,
						Effect.forkChild,
					);
					const invalid = yield* terminal.Resize(0, 24).pipe(Effect.exit);

					yield* terminal.Close;
					yield* terminal.Close;

					const exit = yield* terminal.Exit;

					yield* Fiber.join(output_fiber);

					return {
						exit,
						invalid,
					};
				}),
			).pipe(Effect.provide(NodePtyTerminalDriverLive)),
		);

		expect(error_from(result.invalid)).toMatchObject({
			_tag: "TerminalDriverError",
			operation: "configuration",
		});
		expect(result.exit.reason).toBe("closed");
	});

	it("fails closed when a slow output consumer lets the driver buffer overflow", async () => {
		const exit = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const terminal = yield* open_node_terminal(
						`process.stdout.write("x".repeat(65_536)); setInterval(() => {}, 1_000);`,
					);
					const output_fiber = yield* terminal.Output.pipe(
						Stream.tap(() => Effect.sleep(25)),
						Stream.runDrain,
						Effect.forkChild,
					);
					const exit = yield* terminal.Exit.pipe(
						Effect.timeoutOrElse({
							duration: 5_000,
							orElse: () => Effect.die("terminal overflow did not stop the PTY"),
						}),
					);

					yield* Fiber.join(output_fiber);

					return exit;
				}),
			).pipe(
				Effect.provide(
					make_node_pty_terminal_driver_layer({
						output_capacity: 1,
						output_chunk_bytes: 1_024,
					}),
				),
			),
		);

		expect(exit.reason).toBe("output_overflow");
	});

	it("waits for the native process to exit when its owner scope closes", async () => {
		const pid = await Effect.runPromise(
			Effect.scoped(
				open_node_terminal(`setInterval(() => {}, 1000);`).pipe(
					Effect.map((terminal) => terminal.pid),
				),
			).pipe(Effect.provide(NodePtyTerminalDriverLive)),
		);

		await wait_for_process_exit(pid);

		expect(is_process_alive(pid)).toBe(false);
	});
});

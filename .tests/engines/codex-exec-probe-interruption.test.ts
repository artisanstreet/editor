import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { ProbeCodexExecVersion } from "../../modules/engines/src/codex/internal/codex-exec-probe";
import type { CodexProcessFactory } from "../../modules/engines/src/codex/codex-process";

describe("Codex exec probe interruption", () => {
	it("returns both async iterators and closes the process when the Effect timeout wins", async () => {
		let closed = false;
		let stderr_returned = false;
		let stdout_returned = false;

		const WaitingStream = (on_return: () => void): AsyncIterable<Uint8Array> => ({
			[Symbol.asyncIterator]() {
				return {
					next: () => new Promise<IteratorResult<Uint8Array>>(() => undefined),
					return: async () => {
						on_return();
						return { done: true, value: undefined };
					},
				};
			},
		});
		const factory = {
			Spawn: () =>
				Effect.succeed({
					Close: Effect.sync(() => {
						closed = true;
					}),
					EndInput: Effect.void,
					Exit: Effect.never,
					Kill: () => Effect.void,
					Stderr: WaitingStream(() => {
						stderr_returned = true;
					}),
					Stdout: WaitingStream(() => {
						stdout_returned = true;
					}),
					Write: () => Effect.void,
				}),
		} satisfies typeof CodexProcessFactory.Service;

		await expect(
			Effect.runPromise(
				ProbeCodexExecVersion({
					executable: "codex",
					executable_args: [],
					factory,
					max_stderr_bytes: 64,
					max_stdout_bytes: 64,
					timeout_ms: 10,
				}),
			),
		).rejects.toMatchObject({ _tag: "EngineProbeTimeoutError" });

		expect(closed).toBe(true);
		expect(stderr_returned).toBe(true);
		expect(stdout_returned).toBe(true);
	});
});

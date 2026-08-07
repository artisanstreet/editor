import { Effect } from "effect";
import { describe, expect, it } from "vitest";
import { MakeScopedAttachmentRunner } from "../../modules/frontend/src/lib/lifecycle/scoped-attachment-runner";

describe("scoped attachment runner", () => {
	it("interrupts a delayed visibility fetch before it can delay false or cleanup", async () => {
		const started: string[] = [];
		const cancelled: string[] = [];
		const completed: string[] = [];

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const runner = yield* MakeScopedAttachmentRunner((visibility: boolean) =>
						Effect.gen(function* () {
							if (!visibility) {
								completed.push("hidden");
								return;
							}
							started.push("visible");
							yield* Effect.addFinalizer(() =>
								Effect.gen(function* () {
									yield* Effect.void;
									cancelled.push("visible");
								}),
							);
							yield* Effect.never;
						}),
					);

					runner.ReplaceUnsafe("image:1", true);
					yield* Effect.yieldNow;
					runner.ReplaceUnsafe("image:1", false);
					yield* Effect.sleep("10 millis");
				}),
			),
		);

		expect(started).toEqual(["visible"]);
		expect(cancelled).toEqual(["visible"]);
		expect(completed).toEqual(["hidden"]);
	});

	it("coalesces a synchronous same-key storm to its newest command", async () => {
		const runs: number[] = [];

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const runner = yield* MakeScopedAttachmentRunner((input: number) =>
						Effect.gen(function* () {
							yield* Effect.void;
							runs.push(input);
						}),
					);

					for (let input = 0; input < 10_000; input += 1)
						runner.ReplaceUnsafe("observer:one", input);
					yield* Effect.sleep("10 millis");
				}),
			),
		);

		expect(runs).toEqual([9_999]);
	});

	it("keeps distinct keys and lets the newest release or run win per key", async () => {
		const runs: string[] = [];

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const runner = yield* MakeScopedAttachmentRunner((input: string) =>
						Effect.gen(function* () {
							yield* Effect.void;
							runs.push(input);
						}),
					);

					runner.ReplaceUnsafe("first", "discarded");
					yield* runner.Release("first");
					runner.ReplaceUnsafe("second", "retained");
					runner.ReplaceUnsafe("third", "before-release");
					yield* runner.Release("third");
					runner.ReplaceUnsafe("third", "after-release");
					yield* Effect.sleep("10 millis");
				}),
			),
		);

		expect(runs).toEqual(expect.arrayContaining(["retained", "after-release"]));
		expect(runs).not.toContain("discarded");
		expect(runs).not.toContain("before-release");
	});
});

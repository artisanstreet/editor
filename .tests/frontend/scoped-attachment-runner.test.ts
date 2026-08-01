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
});

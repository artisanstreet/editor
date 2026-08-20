import { Deferred, Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	MakeComposerGestureIntake,
	type ComposerGesture,
} from "../../modules/frontend/src/lib/composer/gesture-intake";

const ImageFile = (name: string): File => ({ name, size: 1, type: "image/png" }) as File;

const Paste = (files: ReadonlyArray<File>) => {
	let prevented = false;
	return {
		event: {
			clipboardData: { files },
			preventDefault: () => {
				prevented = true;
			},
		} as never,
		was_prevented: () => prevented,
	};
};

describe("composer gesture intake retention", () => {
	it("coalesces auto-repeat submits yet accepts one new submit after the first is taken", async () => {
		const observed: Array<ComposerGesture> = [];

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const first_started = yield* Deferred.make<void>();
					const release_first = yield* Deferred.make<void>();
					const second_finished = yield* Deferred.make<void>();
					let submit_count = 0;
					const intake = yield* MakeComposerGestureIntake((gesture) =>
						Effect.gen(function* () {
							observed.push(gesture);
							if (gesture._tag !== "submit") return;
							submit_count += 1;
							if (submit_count === 1) {
								yield* Deferred.succeed(first_started, undefined);
								yield* Deferred.await(release_first);
								return;
							}
							yield* Deferred.succeed(second_finished, undefined);
						}),
					);
					const enter = {
						key: "Enter",
						isComposing: false,
						shiftKey: false,
						preventDefault() {},
					} as never;

					for (let index = 0; index < 64; index += 1) intake.SubmitKey(enter);
					yield* Deferred.await(first_started);
					for (let index = 0; index < 64; index += 1) intake.SubmitKey(enter);
					yield* Deferred.succeed(release_first, undefined);
					yield* Deferred.await(second_finished);
				}),
			),
		);

		expect(observed.map((gesture) => gesture._tag)).toEqual(["submit", "submit"]);
	});

	it("retains every image gesture while its consumer is stalled", async () => {
		const observed: Array<string> = [];
		const burst_size = 128;

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const first_started = yield* Deferred.make<void>();
					const release_first = yield* Deferred.make<void>();
					const completed = yield* Deferred.make<void>();
					const intake = yield* MakeComposerGestureIntake((gesture) =>
						Effect.gen(function* () {
							if (gesture._tag !== "images") return;
							observed.push(gesture.files[0]!.name);
							if (observed.length === 1) {
								yield* Deferred.succeed(first_started, undefined);
								yield* Deferred.await(release_first);
							}
							if (observed.length === burst_size)
								yield* Deferred.succeed(completed, undefined);
						}),
					);

					const pastes = Array.from({ length: burst_size }, (_, index) =>
						Paste([ImageFile(`image-${index}.png`)]),
					);
					for (const paste of pastes) intake.Paste(paste.event);
					expect(pastes.every((paste) => paste.was_prevented())).toBe(true);
					yield* Deferred.await(first_started);
					yield* Deferred.succeed(release_first, undefined);
					yield* Deferred.await(completed);
				}),
			),
		);

		expect(observed).toEqual(
			Array.from({ length: burst_size }, (_, index) => `image-${index}.png`),
		);
	});

	it("continues with retained gestures after one image handler fails", async () => {
		const observed: Array<string> = [];

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const completed = yield* Deferred.make<void>();
					const intake = yield* MakeComposerGestureIntake((gesture) =>
						Effect.gen(function* () {
							if (gesture._tag !== "images") return;
							const name = gesture.files[0]!.name;
							observed.push(name);
							if (name === "failure.png") return yield* Effect.fail("reader failed");
							yield* Deferred.succeed(completed, undefined);
						}),
					);

					intake.Paste(Paste([ImageFile("failure.png")]).event);
					intake.Paste(Paste([ImageFile("success.png")]).event);
					yield* Deferred.await(completed);
				}),
			),
		);

		expect(observed).toEqual(["failure.png", "success.png"]);
	});
});

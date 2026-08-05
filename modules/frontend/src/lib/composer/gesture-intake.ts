import { Effect, Queue } from "effect";

/** Where a drop landed, read before the event's coordinates leave the handler. */
export interface ComposerDropPoint {
	readonly x: number;
	readonly y: number;
}

/** One captured composer gesture, already detached from its expired event. */
export type ComposerGesture =
	| { readonly _tag: "submit" }
	| {
			readonly _tag: "images";
			readonly files: ReadonlyArray<File>;
			readonly point?: ComposerDropPoint;
	  };

/** Composer intake accepts exactly the image types the wire contract carries. */
const ImageFilesIn = (transfer: DataTransfer | null) =>
	[...(transfer?.files ?? [])].filter((file) => file.type.startsWith("image/"));

/**
 * The one composer ingress SER cannot own end to end. A clipboard or drag
 * DataTransfer is readable only while its event is dispatching, and
 * `preventDefault` counts only inside that same window, whereas a SER event
 * effect starts afterwards on a fresh fiber — it would read an empty transfer
 * and prevent nothing. This module is the typed owner of that boundary: its
 * handlers are ordinary synchronous Svelte handlers that capture the gesture
 * and hand it to one worker fiber, which runs the effectful half in gesture
 * order so a send can never overtake the attachment it was typed under.
 */
export const MakeComposerGestureIntake = <Requirements>(
	Run: (gesture: ComposerGesture) => Effect.Effect<void, never, Requirements>,
) =>
	Effect.gen(function* () {
		const gestures = yield* Queue.unbounded<ComposerGesture>();

		yield* Effect.gen(function* () {
			while (true) {
				const gesture = yield* Queue.take(gestures);
				yield* Run(gesture);
			}
		}).pipe(Effect.forkScoped);

		return {
			/** Accepts a file drag so the browser will deliver its drop at all. */
			AcceptFileDrag: (event: DragEvent) => {
				if ([...(event.dataTransfer?.types ?? [])].includes("Files")) {
					event.preventDefault();
				}
			},
			/** Takes dropped images with the point they were released over. */
			Drop: (event: DragEvent) => {
				const files = ImageFilesIn(event.dataTransfer);
				if (files.length === 0) return;
				event.preventDefault();
				Queue.offerUnsafe(gestures, {
					_tag: "images",
					files,
					point: { x: event.clientX, y: event.clientY },
				});
			},
			/** Takes pasted images, keeping the paste itself out of the editor. */
			Paste: (event: ClipboardEvent) => {
				const files = ImageFilesIn(event.clipboardData);
				if (files.length === 0) return;
				event.preventDefault();
				Queue.offerUnsafe(gestures, { _tag: "images", files });
			},
			/** Treats a plain Enter as a send, suppressing its newline in time. */
			SubmitKey: (event: KeyboardEvent) => {
				if (event.isComposing || event.key !== "Enter" || event.shiftKey) return;
				event.preventDefault();
				Queue.offerUnsafe(gestures, { _tag: "submit" });
			},
		};
	});

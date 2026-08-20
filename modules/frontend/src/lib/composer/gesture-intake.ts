import { Effect, Queue } from "effect";

/** Where a drop landed, read before the event's coordinates leave the handler. */
export interface ComposerDropPoint {
	readonly x: number;
	readonly y: number;
}

/** Browser gesture fields consumed synchronously before the event leaves its handler. */
interface ComposerFileTransfer {
	readonly files: Iterable<File>;
	readonly types: Iterable<string>;
}

interface ComposerDragEvent {
	readonly clientX: number;
	readonly clientY: number;
	readonly dataTransfer: ComposerFileTransfer | null;
	preventDefault(): void;
}

interface ComposerPasteEvent {
	readonly clipboardData: ComposerFileTransfer | null;
	preventDefault(): void;
}

interface ComposerSubmitKeyEvent {
	readonly isComposing: boolean;
	readonly key: string;
	readonly shiftKey: boolean;
	preventDefault(): void;
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
const ImageFilesIn = (transfer: ComposerFileTransfer | null) =>
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
export const MakeComposerGestureIntake = <Requirements, Failure>(
	Run: (gesture: ComposerGesture) => Effect.Effect<void, Failure, Requirements>,
) =>
	Effect.gen(function* () {
		const gestures = yield* Queue.unbounded<ComposerGesture>();
		let submit_queued = false;

		yield* Effect.addFinalizer(() => Queue.shutdown(gestures).pipe(Effect.asVoid));

		const MarkGestureTaken = (gesture: ComposerGesture) => {
			if (gesture._tag === "submit") submit_queued = false;
		};

		/** Browser auto-repeat represents one submit intent until the worker takes it. */
		const EnqueueSubmit = () => {
			if (submit_queued) return;
			if (Queue.offerUnsafe(gestures, { _tag: "submit" })) submit_queued = true;
		};

		yield* Effect.gen(function* () {
			while (true) {
				const gesture = yield* Queue.take(gestures);
				MarkGestureTaken(gesture);
				/** One failed gesture must not silence later retained work. */
				yield* Run(gesture).pipe(
					Effect.catch(() =>
						Effect.gen(function* () {
							return;
						}),
					),
				);
			}
		}).pipe(Effect.forkScoped);

		const EnqueueImages = (files: ReadonlyArray<File>, point?: ComposerDropPoint) => {
			const image_gesture: ComposerGesture =
				point === undefined ? { _tag: "images", files } : { _tag: "images", files, point };
			Queue.offerUnsafe(gestures, image_gesture);
		};

		return {
			/** Accepts a file drag so the browser will deliver its drop at all. */
			AcceptFileDrag: (event: ComposerDragEvent) => {
				if ([...(event.dataTransfer?.types ?? [])].includes("Files")) {
					event.preventDefault();
				}
			},
			/** Takes dropped images with the point they were released over. */
			Drop: (event: ComposerDragEvent) => {
				const files = ImageFilesIn(event.dataTransfer);
				if (files.length === 0) return;
				event.preventDefault();
				EnqueueImages(files, { x: event.clientX, y: event.clientY });
			},
			/** Takes pasted images, keeping the paste itself out of the editor. */
			Paste: (event: ComposerPasteEvent) => {
				const files = ImageFilesIn(event.clipboardData);
				if (files.length === 0) return;
				event.preventDefault();
				EnqueueImages(files);
			},
			/** Treats a plain Enter as a send, suppressing its newline in time. */
			SubmitKey: (event: ComposerSubmitKeyEvent) => {
				if (event.isComposing || event.key !== "Enter" || event.shiftKey) return;
				event.preventDefault();
				EnqueueSubmit();
			},
		};
	});

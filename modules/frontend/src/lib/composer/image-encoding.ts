import { Data, Effect } from "effect";

import { RunBrowserDom } from "../browser/dom";
import type { ImageMediaType } from "@artisan/protocol";
import { BestImageFormat, ImageRescaleTarget, type ImageDimensions } from "./image-policy";

/**
 * High enough that screenshot text stays crisp. The provider guidance is
 * explicit that heavy lossy compression damages exactly the content people
 * paste most — text — so this trades a smaller win for a safe one.
 */
const ImageEncodeQuality = 0.92;

/** An image could not be decoded or re-encoded at the canvas boundary. */
export class ImageRescaleFailure extends Data.TaggedError("ImageRescaleFailure")<{
	readonly cause: unknown;
}> {}

/** Maps one fallible foreign image call into this boundary's stable error. */
const RunImageBoundary = <Value>(operation: () => Promise<Value>) =>
	Effect.gen(function* () {
		return yield* Effect.tryPromise({
			catch: (cause) => new ImageRescaleFailure({ cause }),
			try: operation,
		});
	});

const RunImageCanvas = <Value>(operation: () => Value) =>
	Effect.gen(function* () {
		return yield* RunBrowserDom(operation).pipe(
			Effect.mapError((cause) => new ImageRescaleFailure({ cause })),
		);
	});

/** Releases the decoded frame's memory as soon as its pixels are drawn. */
const CloseFrame = (frame: ImageBitmap) =>
	Effect.gen(function* () {
		yield* RunImageCanvas(() => frame.close()).pipe(Effect.ignore);
	});

/** Draws the decoded frame at its target size, ready to be encoded. */
const DrawFrame = (frame: ImageBitmap, target: ImageDimensions) =>
	Effect.gen(function* () {
		return yield* RunImageCanvas(() => {
			const surface = new OffscreenCanvas(target.width, target.height);
			const context = surface.getContext("2d");
			if (context === null) throw new Error("Canvas 2D context is unavailable.");
			context.imageSmoothingEnabled = true;
			context.imageSmoothingQuality = "high";
			context.drawImage(frame, 0, 0, target.width, target.height);
			return surface;
		});
	});

/**
 * A codec the browser cannot encode is silently substituted for PNG rather than
 * refused, so the produced type is checked instead of assumed.
 */
const EncodeCanvas = (canvas: OffscreenCanvas, format: ImageMediaType) =>
	Effect.gen(function* () {
		const encoded = yield* RunImageBoundary(() =>
			canvas.convertToBlob({ quality: ImageEncodeQuality, type: format }),
		);
		return encoded.type === format ? encoded : undefined;
	});

const NameForFormat = (name: string, format: ImageMediaType) =>
	`${name.replace(/\.[^./\\]+$/u, "")}.${format.slice("image/".length)}`;

/**
 * Brings an image within the resolution cap and re-encodes it in the best
 * format its harness accepts. The original file is returned untouched whenever
 * the work would not pay for itself — an image already inside the cap in an
 * already-good format, an encode the browser cannot perform, or a result no
 * smaller than what arrived. A GIF is never touched: canvas keeps only its
 * first frame, and silently discarding an animation is worse than sending bytes
 * the engine already accepts.
 */
export const EncodeComposerImage = (
	file: File,
	media_type: ImageMediaType,
	engine_id: string | undefined,
) =>
	Effect.gen(function* () {
		if (media_type === "image/gif") return file;
		const format = BestImageFormat(engine_id);
		const frame = yield* RunImageBoundary(() => createImageBitmap(file));
		const target =
			ImageRescaleTarget({ height: frame.height, width: frame.width }) ??
			({ height: frame.height, width: frame.width } satisfies ImageDimensions);
		const encoded = yield* Effect.gen(function* () {
			const canvas = yield* DrawFrame(frame, target);
			return yield* EncodeCanvas(canvas, format);
		}).pipe(Effect.ensuring(CloseFrame(frame)));
		if (encoded === undefined || encoded.size >= file.size) return file;
		return new File([encoded], NameForFormat(file.name, format), { type: format });
	});

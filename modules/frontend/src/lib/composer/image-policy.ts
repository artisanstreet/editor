import type { ImageMediaType } from "@artisan/protocol";

/**
 * What intake should do with a pasted image, decided without touching one. The
 * canvas boundary that carries it out lives in `image-encoding.ts`; keeping the
 * rules here makes them answerable without a DOM.
 */

/**
 * Every engine downscales an oversized image before its model sees it: Claude
 * caps the long edge at 2576px on 4.7-and-later models and 1568px elsewhere.
 * Sending more than the cap therefore spends bytes on detail the model never
 * receives, so intake resizes to the highest cap in play. A standard-tier model
 * still resizes what it receives; nothing visible to any model is lost here.
 */
export const MaximumImageLongEdgePixels = 2576;

/**
 * Image codecs ordered worst to best compression at equal perceived quality.
 * AVIF leads the ladder and no harness accepts it yet; it is listed so the day
 * one does, the entry is the whole change.
 */
export const ImageCompressionLadder = [
	"image/png",
	"image/jpeg",
	"image/webp",
	"image/avif",
] as const;

/**
 * What each harness's provider documents as acceptable image input. Claude
 * (JPEG/PNG/GIF/WebP) and OpenAI publish the same four, so WebP is the best
 * either will take — but the tables stay separate, because the day they diverge
 * is the day a shared list would quietly send one of them something it rejects.
 */
const HarnessImageFormats: Record<string, ReadonlyArray<ImageMediaType>> = {
	claude: ["image/gif", "image/jpeg", "image/png", "image/webp"],
	codex: ["image/gif", "image/jpeg", "image/png", "image/webp"],
};

/**
 * What an engine with no table gets: the one format every harness has always
 * taken, and a lossless one. Guessing upward on an unknown engine risks a
 * rejected image; guessing lossy risks a degraded one.
 */
const UniversalImageFormats: ReadonlyArray<ImageMediaType> = ["image/png"];

/** The most compressed format the harness will actually accept. */
export const BestImageFormat = (engine_id: string | undefined): ImageMediaType => {
	const accepted = HarnessImageFormats[engine_id ?? ""] ?? UniversalImageFormats;
	for (let index = ImageCompressionLadder.length - 1; index >= 0; index -= 1) {
		const format = ImageCompressionLadder[index];
		const supported = accepted.find((candidate) => candidate === format);
		if (supported !== undefined) return supported;
	}
	return "image/png";
};

export interface ImageDimensions {
	readonly height: number;
	readonly width: number;
}

/**
 * Preserves aspect ratio while bringing the long edge to the cap, and never
 * enlarges: an image already within the cap is returned untouched so it can
 * keep its original bytes instead of being re-encoded for nothing.
 */
export const ImageRescaleTarget = (
	source: ImageDimensions,
	long_edge = MaximumImageLongEdgePixels,
): ImageDimensions | undefined => {
	const longest = Math.max(source.width, source.height);
	if (!Number.isFinite(longest) || longest <= long_edge) return undefined;
	const scale = long_edge / longest;
	return {
		height: Math.max(1, Math.round(source.height * scale)),
		width: Math.max(1, Math.round(source.width * scale)),
	};
};

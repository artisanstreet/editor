import { nativeImage } from "electron";
import type { NativeImage } from "electron";

import {
	AttentionOverlayLabelFor,
	attention_overlay_sources,
	type AttentionOverlayLabel,
} from "./catalog";

/**
 * Ten labels at most, so the decoded images simply live for the process:
 * re-decoding three PNGs on every count change would be waste, and an entry
 * can never be stale because the pixels for a label never change.
 */
const decoded = new Map<AttentionOverlayLabel, NativeImage>();

export const AttentionOverlayImage = (count: number): NativeImage => {
	const label = AttentionOverlayLabelFor(count);
	const cached = decoded.get(label);
	if (cached !== undefined) return cached;

	const source = attention_overlay_sources[label];
	const image = nativeImage.createEmpty();
	image.addRepresentation({ buffer: Buffer.from(source.x1, "base64"), scaleFactor: 1 });
	image.addRepresentation({ buffer: Buffer.from(source.x1_5, "base64"), scaleFactor: 1.5 });
	image.addRepresentation({ buffer: Buffer.from(source.x2, "base64"), scaleFactor: 2 });
	decoded.set(label, image);

	return image;
};

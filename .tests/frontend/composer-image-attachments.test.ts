import { describe, expect, it } from "vitest";

import {
	MaximumImageAttachmentCount as ProtocolMaximumCount,
	MaximumImageAttachmentTotalBytes as ProtocolMaximumTotalBytes,
} from "../../modules/protocol/src/attachments";
import {
	BestImageFormat,
	ImageCompressionLadder,
	ImageRescaleTarget,
	MaximumImageLongEdgePixels,
} from "../../modules/frontend/src/lib/composer/image-policy";
import {
	FindDuplicateImageAttachment,
	IsDuplicateImageAttachment,
	MakeImageAttachmentParts,
	MakeUserMessageContent,
	MaximumImageAttachmentBytes,
	MaximumImageAttachmentCount,
	ValidateImageAttachmentBatch,
	ValidateImageAttachmentCandidates,
} from "../../modules/frontend/src/lib/composer/image-attachments";

const MakeAttachment = (
	id: string,
	content_base64: string,
	size_bytes = content_base64.length,
) => ({
	content_base64,
	id,
	mime_type: "image/png" as const,
	name: `${id}.png`,
	preview_url: `blob:${id}`,
	ready: true,
	size_bytes,
	source_digest: `digest-${content_base64}`,
	source_size_bytes: size_bytes,
});

describe("composer image attachments", () => {
	it("refuses what the command boundary would refuse", () => {
		expect(MaximumImageAttachmentCount).toBe(ProtocolMaximumCount);
	});

	it("rejects unsupported images and per-file limits before any file is read", () => {
		expect(
			ValidateImageAttachmentCandidates([
				{ name: "drawing.svg", size_bytes: 1, type: "image/svg+xml" },
			]),
		).toContain("JPEG, PNG, WebP, or GIF");
		expect(
			ValidateImageAttachmentCandidates([
				{
					name: "large.png",
					size_bytes: MaximumImageAttachmentBytes + 1,
					type: "image/png",
				},
			]),
		).toContain("5 MiB");
		expect(
			ValidateImageAttachmentCandidates(
				Array.from({ length: MaximumImageAttachmentCount + 1 }, () => ({
					name: "x.png",
					size_bytes: 1,
					type: "image/png",
				})),
			),
		).toContain(`up to ${MaximumImageAttachmentCount}`);
		/** A full batch is only refused once duplicates have been resolved. */
		expect(
			ValidateImageAttachmentCandidates(
				Array.from({ length: MaximumImageAttachmentCount }, () => ({
					name: "x.png",
					size_bytes: MaximumImageAttachmentBytes,
					type: "image/png",
				})),
			),
		).toBeUndefined();
	});

	it("rejects a batch that cannot fit beside what is already attached", () => {
		expect(
			ValidateImageAttachmentBatch(
				Array.from({ length: MaximumImageAttachmentCount }, () => ({ size_bytes: 1 })),
				[{ size_bytes: 1 }],
			),
		).toContain(`up to ${MaximumImageAttachmentCount}`);
		expect(
			ValidateImageAttachmentBatch(
				[{ size_bytes: ProtocolMaximumTotalBytes - 1 }],
				[{ size_bytes: 2 }],
			),
		).toContain("12 MiB");
		expect(ValidateImageAttachmentBatch([{ size_bytes: 1 }], [])).toBeUndefined();
		expect(
			ValidateImageAttachmentBatch([{ size_bytes: 1 }], [{ size_bytes: 1 }]),
		).toBeUndefined();
	});

	it("encodes in the most compressed format each harness actually accepts", () => {
		/** The ladder runs worst to best, so the last entry is the best available. */
		expect(ImageCompressionLadder[0]).toBe("image/png");
		expect(ImageCompressionLadder.at(-1)).toBe("image/avif");
		/** No harness accepts AVIF yet, so WebP is the best either will take. */
		expect(BestImageFormat("claude")).toBe("image/webp");
		expect(BestImageFormat("codex")).toBe("image/webp");
		/** An engine with no published table gets what every harness accepts. */
		expect(BestImageFormat("unknown-harness")).toBe("image/png");
		expect(BestImageFormat(undefined)).toBe("image/png");
	});

	it("holds pending attachments out of the wire parts and duplicate checks", () => {
		const pending = { ...MakeAttachment("pending", "AAAA"), content_base64: "", ready: false };
		const ready = MakeAttachment("ready", "AAAA");
		expect(
			MakeImageAttachmentParts(new Map([[pending.id, pending]]), [
				{ id: pending.id, position: 0 },
			]),
		).toEqual([]);
		/** Two images still encoding are not "the same image". */
		expect(IsDuplicateImageAttachment([pending], { ...pending, id: "other" })).toBe(false);
		expect(IsDuplicateImageAttachment([ready], { ...pending, id: "other" })).toBe(false);
	});

	it("shrinks only what exceeds the engines' own resolution cap", () => {
		/** A 4K screenshot: every engine downscales it, so intake does it first. */
		expect(ImageRescaleTarget({ height: 2160, width: 3840 })).toEqual({
			height: 1449,
			width: MaximumImageLongEdgePixels,
		});
		expect(ImageRescaleTarget({ height: 3840, width: 2160 })).toEqual({
			height: MaximumImageLongEdgePixels,
			width: 1449,
		});
		/** Within the cap the original bytes are kept — no re-encode. */
		expect(ImageRescaleTarget({ height: 1080, width: 1920 })).toBeUndefined();
		expect(
			ImageRescaleTarget({
				height: MaximumImageLongEdgePixels,
				width: MaximumImageLongEdgePixels,
			}),
		).toBeUndefined();
		/** A sliver never rounds away to nothing. */
		expect(ImageRescaleTarget({ height: 1, width: 10_000 })).toEqual({
			height: 1,
			width: MaximumImageLongEdgePixels,
		});
	});

	it("recognises a re-paste from the pasted file, before anything is shown", () => {
		const attached = MakeAttachment("first", "AAAA");
		/** A second paste of one capture is a fresh File with the same bytes. */
		expect(
			FindDuplicateImageAttachment([attached], {
				source_digest: attached.source_digest,
				source_size_bytes: attached.source_size_bytes,
			}),
		).toBe(attached);
		expect(
			FindDuplicateImageAttachment([attached], {
				source_digest: "digest-BBBB",
				source_size_bytes: attached.source_size_bytes,
			}),
		).toBeUndefined();
		/** Without a digest nothing matches here; the encoded check still stands. */
		expect(
			FindDuplicateImageAttachment([attached], {
				source_digest: "",
				source_size_bytes: attached.source_size_bytes,
			}),
		).toBeUndefined();
	});

	it("recognises a re-pasted image by its bytes, not its file identity", () => {
		const attached = MakeAttachment("first", "AAAA");
		/** A second paste of one capture arrives with a fresh id and name. */
		expect(IsDuplicateImageAttachment([attached], MakeAttachment("second", "AAAA"))).toBe(true);
		expect(IsDuplicateImageAttachment([attached], MakeAttachment("second", "BBBB"))).toBe(
			false,
		);
		/** Equal length is not equal content. */
		expect(
			IsDuplicateImageAttachment([attached], {
				...MakeAttachment("second", "BBBB"),
				size_bytes: attached.size_bytes,
			}),
		).toBe(false);
		expect(IsDuplicateImageAttachment([], attached)).toBe(false);
	});

	it("preserves document token order and exact text positions in wire attachments", () => {
		const attachment = {
			content_base64: "aGVsbG8=",
			id: "image-1",
			mime_type: "image/png" as const,
			name: "design.png",
			preview_url: "blob:image-1",
			ready: true,
			size_bytes: 5,
			source_digest: "digest-image-1",
			source_size_bytes: 9,
		};
		expect(
			MakeImageAttachmentParts(new Map([[attachment.id, attachment]]), [
				{ id: "image-1", position: 12 },
			]),
		).toEqual(
			[
				{ ...attachment, position: 12 },
				// Preview URL, readiness, and the pre-encoding size are renderer-only
				// bookkeeping and must never cross the command boundary.
			].map(
				({
					preview_url: _preview_url,
					ready: _ready,
					source_digest: _source_digest,
					source_size_bytes: _source_size_bytes,
					...part
				}) => part,
			),
		);
	});

	it("rebuilds text and image parts in authored order", () => {
		expect(
			MakeUserMessageContent("FirstSecondThird", [
				{ id: "one", position: 5 },
				{ id: "two", position: 11 },
			]),
		).toEqual([
			{ text: "First", type: "text" },
			{ client_token: "one", type: "image" },
			{ text: "Second", type: "text" },
			{ client_token: "two", type: "image" },
			{ text: "Third", type: "text" },
		]);
	});
});

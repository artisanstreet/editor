import { describe, expect, it } from "vitest";

import {
	MakeImageAttachmentParts,
	MakeUserMessageContent,
	MaximumImageAttachmentBytes,
	ValidateImageAttachmentCandidates,
} from "../../modules/frontend/src/lib/composer/image-attachments";

describe("composer image attachments", () => {
	it("rejects unsupported images, per-file limits, count limits, and oversized batches", () => {
		expect(
			ValidateImageAttachmentCandidates(
				[],
				[{ name: "drawing.svg", size_bytes: 1, type: "image/svg+xml" }],
			),
		).toContain("JPEG, PNG, WebP, or GIF");
		expect(
			ValidateImageAttachmentCandidates(
				[],
				[
					{
						name: "large.png",
						size_bytes: MaximumImageAttachmentBytes + 1,
						type: "image/png",
					},
				],
			),
		).toContain("5 MiB");
		expect(
			ValidateImageAttachmentCandidates(
				[],
				Array.from({ length: 5 }, () => ({
					name: "x.png",
					size_bytes: 1,
					type: "image/png",
				})),
			),
		).toContain("up to 4");
		expect(
			ValidateImageAttachmentCandidates(
				[],
				[
					{ name: "a.png", size_bytes: 5 * 1024 * 1024, type: "image/png" },
					{ name: "b.png", size_bytes: 4 * 1024 * 1024, type: "image/png" },
				],
			),
		).toContain("8 MiB");
	});

	it("preserves document token order and exact text positions in wire attachments", () => {
		const attachment = {
			content_base64: "aGVsbG8=",
			id: "image-1",
			mime_type: "image/png" as const,
			name: "design.png",
			preview_url: "blob:image-1",
			size_bytes: 5,
		};
		expect(
			MakeImageAttachmentParts(new Map([[attachment.id, attachment]]), [
				{ id: "image-1", position: 12 },
			]),
		).toEqual(
			[
				{ ...attachment, position: 12 },
				// Preview URLs are renderer-only and must never cross the command boundary.
			].map(({ preview_url: _preview_url, ...part }) => part),
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
			{ attachment_id: "one", type: "image" },
			{ text: "Second", type: "text" },
			{ attachment_id: "two", type: "image" },
			{ text: "Third", type: "text" },
		]);
	});
});

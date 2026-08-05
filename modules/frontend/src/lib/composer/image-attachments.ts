import {
	MaximumImageAttachmentBytes,
	MaximumImageAttachmentCount,
	MaximumImageAttachmentTotalBytes,
} from "@artisan/protocol";

export const SupportedImageMimeTypes = new Set([
	"image/gif",
	"image/jpeg",
	"image/png",
	"image/webp",
]);

/** The composer refuses exactly what the command boundary would refuse. */
export {
	MaximumImageAttachmentBytes,
	MaximumImageAttachmentCount,
	MaximumImageAttachmentTotalBytes,
};

/**
 * An attachment exists before its bytes do. Intake shows the image the moment
 * it is pasted and encodes it behind that preview, so `content_base64` and
 * `size_bytes` describe the encoded image and are only meaningful once `ready`.
 * `preview_url` always points at the original: what the user pasted is what
 * they should see, whatever the wire ends up carrying.
 */
export interface ComposerImageAttachment {
	readonly content_base64: string;
	readonly id: string;
	readonly mime_type: "image/gif" | "image/jpeg" | "image/png" | "image/webp";
	readonly name: string;
	readonly preview_url: string;
	readonly ready: boolean;
	readonly size_bytes: number;
	/** Digest of the pasted file, so a re-paste is recognised before it is shown. */
	readonly source_digest: string;
	/** The pasted file's own size, before encoding shrinks it. */
	readonly source_size_bytes: number;
}

export interface ComposerImageAttachmentPart {
	readonly content_base64: string;
	readonly id: string;
	readonly mime_type: ComposerImageAttachment["mime_type"];
	readonly name: string;
	readonly position: number;
	readonly size_bytes: number;
}

export interface ComposerSubmission {
	readonly attachments: ReadonlyArray<ComposerImageAttachmentPart>;
	readonly text: string;
}

export interface ImageAttachmentCandidate {
	readonly name: string;
	readonly size_bytes: number;
	readonly type: string;
}

export const IsSupportedImageMimeType = (
	mime_type: string,
): mime_type is ComposerImageAttachment["mime_type"] => SupportedImageMimeTypes.has(mime_type);

/**
 * Rejects offered files on their metadata alone, before any of them is read.
 * Whether the batch fits is settled later, in `ValidateImageAttachmentBatch`:
 * duplicates only become visible once bytes exist, so counting here would
 * refuse a paste that adds nothing. The batch itself is still bounded, so a
 * dropped folder cannot pull an unbounded amount of file content into memory.
 */
export const ValidateImageAttachmentCandidates = (
	candidates: ReadonlyArray<ImageAttachmentCandidate>,
): string | undefined => {
	if (candidates.length === 0) return undefined;
	if (candidates.length > MaximumImageAttachmentCount) {
		return `Attach up to ${MaximumImageAttachmentCount} images at a time.`;
	}

	for (const candidate of candidates) {
		if (!IsSupportedImageMimeType(candidate.type)) {
			return `${candidate.name || "That file"} is not a JPEG, PNG, WebP, or GIF image.`;
		}
		if (candidate.size_bytes > MaximumImageAttachmentBytes) {
			return `${candidate.name || "That image"} exceeds the 5 MiB limit.`;
		}
	}
};

/** Validates what would actually be attached, once duplicates are already out. */
export const ValidateImageAttachmentBatch = (
	existing: ReadonlyArray<Pick<ComposerImageAttachment, "size_bytes">>,
	additions: ReadonlyArray<Pick<ComposerImageAttachment, "size_bytes">>,
): string | undefined => {
	if (additions.length === 0) return undefined;
	if (existing.length + additions.length > MaximumImageAttachmentCount) {
		return `Attach up to ${MaximumImageAttachmentCount} images at a time.`;
	}
	if (
		[...existing, ...additions].reduce((total, item) => total + item.size_bytes, 0) >
		MaximumImageAttachmentTotalBytes
	) {
		return "Attached images together cannot exceed 12 MiB.";
	}
};

/**
 * Finds the attachment a pasted file is a re-paste of, before anything is shown.
 * Identity is the file's own bytes: one clipboard capture pasted twice arrives
 * as a fresh File with its own name and timestamp, so only a digest recognises
 * it. Without a digest nothing matches here and the slower check below stands.
 */
export const FindDuplicateImageAttachment = (
	existing: ReadonlyArray<ComposerImageAttachment>,
	candidate: Pick<ComposerImageAttachment, "source_digest" | "source_size_bytes">,
): ComposerImageAttachment | undefined =>
	candidate.source_digest === ""
		? undefined
		: existing.find(
				(attachment) =>
					attachment.source_size_bytes === candidate.source_size_bytes &&
					attachment.source_digest === candidate.source_digest,
			);

/**
 * The encoded-bytes check, kept for the case where no digest was available:
 * two files that encode to the same image are still one attachment.
 */
export const IsDuplicateImageAttachment = (
	existing: ReadonlyArray<ComposerImageAttachment>,
	candidate: ComposerImageAttachment,
): boolean =>
	candidate.ready &&
	existing.some(
		(attachment) =>
			attachment.ready &&
			attachment.id !== candidate.id &&
			attachment.size_bytes === candidate.size_bytes &&
			attachment.mime_type === candidate.mime_type &&
			attachment.content_base64 === candidate.content_base64,
	);

/** Produces the ordered wire parts from the editable document's token sequence. */
export const MakeImageAttachmentParts = (
	attachments: ReadonlyMap<string, ComposerImageAttachment>,
	tokens: ReadonlyArray<{ readonly id: string; readonly position: number }>,
): ReadonlyArray<ComposerImageAttachmentPart> =>
	tokens.flatMap(({ id, position }) => {
		const attachment = attachments.get(id);
		/** An attachment still being encoded has no bytes to send yet. */
		return attachment === undefined || !attachment.ready
			? []
			: [
					{
						content_base64: attachment.content_base64,
						id: attachment.id,
						mime_type: attachment.mime_type,
						name: attachment.name,
						position,
						size_bytes: attachment.size_bytes,
					},
				];
	});

/** Rebuilds provider-neutral authored order from visible text and atomic image tokens. */
export const MakeUserMessageContent = (
	text: string,
	tokens: ReadonlyArray<{ readonly id: string; readonly position: number }>,
): ReadonlyArray<
	| { readonly text: string; readonly type: "text" }
	| { readonly client_token: string; readonly type: "image" }
> => {
	const content: Array<
		| { readonly text: string; readonly type: "text" }
		| { readonly client_token: string; readonly type: "image" }
	> = [];
	let cursor = 0;
	for (const token of tokens) {
		const position = Math.max(cursor, Math.min(text.length, token.position));
		const preceding = text.slice(cursor, position);
		if (preceding.length > 0) content.push({ text: preceding, type: "text" });
		content.push({ client_token: token.id, type: "image" });
		cursor = position;
	}
	const trailing = text.slice(cursor);
	if (trailing.length > 0) content.push({ text: trailing, type: "text" });
	return content;
};

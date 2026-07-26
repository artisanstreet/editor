export const SupportedImageMimeTypes = new Set([
	"image/gif",
	"image/jpeg",
	"image/png",
	"image/webp",
]);

export const MaximumImageAttachmentCount = 4;
export const MaximumImageAttachmentBytes = 5 * 1024 * 1024;
export const MaximumImageAttachmentTotalBytes = 8 * 1024 * 1024;

export interface ComposerImageAttachment {
	readonly content_base64: string;
	readonly id: string;
	readonly mime_type: "image/gif" | "image/jpeg" | "image/png" | "image/webp";
	readonly name: string;
	readonly preview_url: string;
	readonly size_bytes: number;
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

/** Validates a complete intake batch before any file contents enter renderer state. */
export const ValidateImageAttachmentCandidates = (
	existing: ReadonlyArray<Pick<ComposerImageAttachment, "size_bytes">>,
	candidates: ReadonlyArray<ImageAttachmentCandidate>,
): string | undefined => {
	if (candidates.length === 0) return undefined;
	if (existing.length + candidates.length > MaximumImageAttachmentCount) {
		return `Attach up to ${MaximumImageAttachmentCount} images at a time.`;
	}

	const all = [...existing, ...candidates];
	for (const candidate of candidates) {
		if (!IsSupportedImageMimeType(candidate.type)) {
			return `${candidate.name || "That file"} is not a JPEG, PNG, WebP, or GIF image.`;
		}
		if (candidate.size_bytes > MaximumImageAttachmentBytes) {
			return `${candidate.name || "That image"} exceeds the 5 MiB limit.`;
		}
	}

	if (
		all.reduce((total, item) => total + item.size_bytes, 0) > MaximumImageAttachmentTotalBytes
	) {
		return "Attached images together cannot exceed 8 MiB.";
	}
};

/** Produces the ordered wire parts from the editable document's token sequence. */
export const MakeImageAttachmentParts = (
	attachments: ReadonlyMap<string, ComposerImageAttachment>,
	tokens: ReadonlyArray<{ readonly id: string; readonly position: number }>,
): ReadonlyArray<ComposerImageAttachmentPart> =>
	tokens.flatMap(({ id, position }) => {
		const attachment = attachments.get(id);
		return attachment === undefined
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
	| { readonly attachment_id: string; readonly type: "image" }
> => {
	const content: Array<
		| { readonly text: string; readonly type: "text" }
		| { readonly attachment_id: string; readonly type: "image" }
	> = [];
	let cursor = 0;
	for (const token of tokens) {
		const position = Math.max(cursor, Math.min(text.length, token.position));
		const preceding = text.slice(cursor, position);
		if (preceding.length > 0) content.push({ text: preceding, type: "text" });
		content.push({ attachment_id: token.id, type: "image" });
		cursor = position;
	}
	const trailing = text.slice(cursor);
	if (trailing.length > 0) content.push({ text: trailing, type: "text" });
	return content;
};

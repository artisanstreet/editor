import { Schema } from "effect";

import { Identifier } from "./common";

export const ImageMediaType = Schema.Literals([
	"image/gif",
	"image/jpeg",
	"image/png",
	"image/webp",
]);
export type ImageMediaType = typeof ImageMediaType.Type;

/**
 * One message's images travel inside a single control frame, so the Forge
 * socket's 16 MiB payload ceiling is what actually bounds them. These budgets
 * keep a full batch comfortably beneath it while leaving a screenshot-sized
 * allowance per image, and they are the sole definition: composer intake,
 * command validation, and durable acceptance all read them from here.
 */
export const MaximumImageAttachmentCount = 10;
export const MaximumImageAttachmentBytes = 5 * 1024 * 1024;
export const MaximumImageAttachmentTotalBytes = 12 * 1024 * 1024;

export const ImageAttachmentBytes = Schema.Uint8Array.check(
	Schema.makeFilter<Uint8Array>((bytes) =>
		bytes.byteLength <= MaximumImageAttachmentBytes
			? undefined
			: "Expected an image no larger than 5 MiB",
	),
);

/**
 * Bytes are accepted only at the authenticated command boundary. The token is
 * request-local and Forge replaces it with a durable attachment identity.
 */
export const ImageAttachmentUpload = Schema.Struct({
	bytes: Schema.optional(ImageAttachmentBytes),
	client_token: Identifier,
	media_type: ImageMediaType,
	name: Schema.String.check(
		Schema.makeFilter<string>((value) =>
			value.length > 0 && value.length <= 255
				? undefined
				: "Expected an image name up to 255 characters",
		),
	),
});
export type ImageAttachmentUpload = typeof ImageAttachmentUpload.Type;

/** Preserves authored ordering while image references are request-local tokens. */
export const UserMessageInputContentPart = Schema.Union([
	Schema.Struct({ text: Schema.String, type: Schema.Literal("text") }),
	Schema.Struct({ client_token: Identifier, type: Schema.Literal("image") }),
]).pipe(Schema.toTaggedUnion("type"));
export type UserMessageInputContentPart = typeof UserMessageInputContentPart.Type;

/** Safe attachment facts exposed in conversation projections and read boundaries. */
export const ImageAttachmentReference = Schema.Struct({
	id: Identifier,
	media_type: ImageMediaType,
	name: Schema.String,
	size_bytes: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
});
export type ImageAttachmentReference = typeof ImageAttachmentReference.Type;

/** Requests one image body through the authenticated thread control boundary. */
export const MessageImageAttachmentQuery = Schema.Struct({
	attachment_id: Identifier,
	thread_id: Identifier,
});
export type MessageImageAttachmentQuery = typeof MessageImageAttachmentQuery.Type;

/** A bounded image body returned only to the authenticated owning thread. */
export const MessageImageAttachment = Schema.Struct({
	bytes: ImageAttachmentBytes,
	id: Identifier,
	media_type: ImageMediaType,
	name: Schema.String,
	size_bytes: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
});
export type MessageImageAttachment = typeof MessageImageAttachment.Type;

/** An absent attachment is an expected read outcome, never a projection error. */
export const MessageImageAttachmentQueryResult = Schema.Union([
	Schema.Struct({ attachment: MessageImageAttachment, status: Schema.Literal("found") }),
	Schema.Struct({ status: Schema.Literal("not_found") }),
]).pipe(Schema.toTaggedUnion("status"));
export type MessageImageAttachmentQueryResult = typeof MessageImageAttachmentQueryResult.Type;

/** Preserves a user's authored text/image ordering without embedding bytes in durable events. */
export const UserMessageContentPart = Schema.Union([
	Schema.Struct({ text: Schema.String, type: Schema.Literal("text") }),
	Schema.Struct({ attachment_id: Identifier, type: Schema.Literal("image") }),
]).pipe(Schema.toTaggedUnion("type"));
export type UserMessageContentPart = typeof UserMessageContentPart.Type;

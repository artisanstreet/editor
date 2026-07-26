import { inArray } from "drizzle-orm";
import { Effect } from "effect";

import { type CommandPayload, type ImageAttachmentReference } from "@artisan/protocol";

import type { DatabaseClient } from "../database";
import { MessageImageAttachments } from "../schema";

export const ImageAttachmentsFor = (
	payload: CommandPayload,
): ReadonlyArray<ImageAttachmentReference> =>
	payload.type === "thread.send_message"
		? (payload.attachments ?? []).map((attachment) => ({
				id: attachment.id,
				media_type: attachment.media_type,
				name: attachment.name,
				size_bytes: attachment.bytes?.byteLength ?? 0,
			}))
		: [];

export const SanitisePayload = (payload: CommandPayload) =>
	payload.type !== "thread.send_message" || payload.attachments === undefined
		? payload
		: {
				...payload,
				attachments: payload.attachments.map(
					({ bytes: _bytes, ...attachment }) => attachment,
				),
			};

const BytesMatchImageType = (
	media_type: "image/gif" | "image/jpeg" | "image/png" | "image/webp",
	bytes: Uint8Array,
) => {
	const ascii = (start: number, end: number) =>
		new TextDecoder("ascii").decode(bytes.subarray(start, end));

	switch (media_type) {
		case "image/gif":
			return bytes.length >= 6 && /^GIF8[79]a$/.test(ascii(0, 6));
		case "image/jpeg":
			return bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff;
		case "image/png":
			return (
				bytes.length >= 8 &&
				[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a].every(
					(byte, index) => bytes[index] === byte,
				)
			);
		case "image/webp":
			return bytes.length >= 12 && ascii(0, 4) === "RIFF" && ascii(8, 12) === "WEBP";
	}
};

export const ValidateImageAttachments = (payload: CommandPayload) => {
	if (payload.type !== "thread.send_message" || payload.attachments === undefined) return;
	const ids = new Set<string>();
	let total_bytes = 0;
	for (const attachment of payload.attachments) {
		if (attachment.bytes === undefined) {
			throw new Error("Image bytes are required when accepting a new message attachment");
		}
		if (!BytesMatchImageType(attachment.media_type, attachment.bytes)) {
			throw new Error(`Image bytes do not match ${attachment.media_type}`);
		}
		if (ids.has(attachment.id)) throw new Error("Message image attachment ids must be unique");
		ids.add(attachment.id);
		total_bytes += attachment.bytes.byteLength;
	}
	if (total_bytes > 8 * 1024 * 1024) throw new Error("Message images may total at most 8 MiB");
	for (const part of payload.content ?? []) {
		if (part.type === "image" && !ids.has(part.attachment_id)) {
			throw new Error("Every image content part must reference an uploaded attachment");
		}
	}
};

export const HydrateImageAttachments = (database: DatabaseClient, payload: CommandPayload) => {
	if (payload.type !== "thread.send_message" || payload.attachments === undefined) {
		return Effect.succeed(payload);
	}
	const attachments = payload.attachments;
	return database
		.select()
		.from(MessageImageAttachments)
		.where(
			inArray(
				MessageImageAttachments.attachment_id,
				attachments.map((attachment) => attachment.id),
			),
		)
		.pipe(
			Effect.flatMap((rows) => {
				const by_id = new Map(rows.map((row) => [row.attachment_id, row]));
				const hydrated = attachments.map((attachment) => {
					const stored = by_id.get(attachment.id);
					if (!stored) {
						throw new Error(`Missing durable image attachment ${attachment.id}`);
					}
					return { ...attachment, bytes: new Uint8Array(stored.content) };
				});
				return Effect.succeed({
					...payload,
					attachments: hydrated,
				});
			}),
		);
};

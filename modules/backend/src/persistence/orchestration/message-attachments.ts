import { inArray } from "drizzle-orm";
import { Effect } from "effect";

import {
	MaximumImageAttachmentTotalBytes,
	type CommandPayload,
	type ImageAttachmentReference,
} from "@artisan/protocol";

import type { DatabaseClient } from "../database";
import { MessageImageAttachments } from "../tables";
import type { AuthoritativeThreadSendMessageCommand } from "./message-command";

export const ImageAttachmentsFor = (
	payload: AuthoritativeThreadSendMessageCommand,
): ReadonlyArray<ImageAttachmentReference> =>
	payload.type === "thread.send_message"
		? (payload.attachments ?? []).map((attachment) => ({
				id: attachment.id,
				media_type: attachment.media_type,
				name: attachment.name,
				size_bytes: attachment.bytes?.byteLength ?? 0,
			}))
		: [];

export const SanitisePayload = (payload: CommandPayload | AuthoritativeThreadSendMessageCommand) =>
	payload.type !== "thread.send_message" || payload.attachments === undefined
		? payload
		: {
				...payload,
				attachments: payload.attachments.map(
					({ bytes: _bytes, ...attachment }) => attachment,
				),
			};

/** Produces token-free, deterministic client intent for idempotency comparison. */
export const CanonicaliseClientMessageIntent = (payload: CommandPayload) => {
	if (payload.type !== "thread.send_message") return SanitisePayload(payload);
	const slots = new Map(
		(payload.attachments ?? []).map((attachment, index) => [attachment.client_token, index]),
	);
	return {
		...payload,
		attachments: (payload.attachments ?? []).map((attachment, index) => ({
			idempotency_slot: index,
			media_type: attachment.media_type,
			name: attachment.name,
		})),
		content: payload.content?.map((part) =>
			part.type === "text"
				? part
				: {
						idempotency_slot: slots.get(part.client_token),
						type: "image" as const,
					},
		),
	};
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

export const ValidateImageAttachments = (
	payload: CommandPayload | AuthoritativeThreadSendMessageCommand,
) => {
	if (payload.type !== "thread.send_message" || payload.attachments === undefined) return;
	const tokens = new Set<string>();
	let total_bytes = 0;
	for (const attachment of payload.attachments) {
		if (attachment.bytes === undefined) {
			throw new Error("Image bytes are required when accepting a new message attachment");
		}
		if (!BytesMatchImageType(attachment.media_type, attachment.bytes)) {
			throw new Error(`Image bytes do not match ${attachment.media_type}`);
		}
		const token = "client_token" in attachment ? attachment.client_token : attachment.id;
		if (tokens.has(token)) {
			throw new Error("Message image attachment client tokens must be unique");
		}
		tokens.add(token);
		total_bytes += attachment.bytes.byteLength;
	}
	if (total_bytes > MaximumImageAttachmentTotalBytes) {
		throw new Error("Message images may total at most 12 MiB");
	}
	for (const part of payload.content ?? []) {
		const token =
			part.type === "image"
				? "client_token" in part
					? part.client_token
					: part.attachment_id
				: undefined;
		if (token !== undefined && !tokens.has(token)) {
			throw new Error("Every image content part must reference an uploaded attachment");
		}
	}
};

export const HydrateImageAttachments = (
	database: DatabaseClient,
	payload: AuthoritativeThreadSendMessageCommand,
) => {
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

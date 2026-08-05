import { SnowflakeId } from "@artisan/protocol";
import { Data, Effect } from "effect";
import { RunBrowserDom } from "$lib/browser/dom";
import { EncodeComposerImage } from "./image-encoding";
import { IsSupportedImageMimeType, type ComposerImageAttachment } from "./image-attachments";

/** A browser FileReader failed before an image could enter composer state. */
export class AttachmentReadError extends Data.TaggedError("AttachmentReadError")<{
	readonly cause: unknown;
	readonly file_name: string;
}> {}

/** Maps a fallible browser primitive into the attachment boundary's stable error. */
const RunAttachmentBrowser = <Value>(file: File, operation: () => Value) =>
	Effect.gen(function* () {
		return yield* RunBrowserDom(operation).pipe(
			Effect.mapError(
				({ cause }) => new AttachmentReadError({ cause, file_name: file.name }),
			),
		);
	});

/** Bridges FileReader's foreign completion callbacks into the Effect error channel. */
const AwaitFileReader = (file: File, reader: FileReader) =>
	Effect.gen(function* () {
		return yield* Effect.callback<string, AttachmentReadError>((resume) => {
			reader.onerror = () =>
				resume(
					Effect.gen(function* () {
						return yield* Effect.fail(
							new AttachmentReadError({
								cause: reader.error,
								file_name: file.name,
							}),
						);
					}),
				);
			reader.onload = () => {
				const source = typeof reader.result === "string" ? reader.result : undefined;
				const content_base64 = source?.slice(source.indexOf(",") + 1);
				if (!content_base64 || !IsSupportedImageMimeType(file.type)) {
					resume(
						Effect.gen(function* () {
							return yield* Effect.fail(
								new AttachmentReadError({
									cause: "invalid-image-data",
									file_name: file.name,
								}),
							);
						}),
					);
					return;
				}
				resume(
					Effect.gen(function* () {
						return content_base64;
					}),
				);
			};
			/** FileReader's callback registration is its foreign completion ingress. */
			reader.readAsDataURL(file);
			return Effect.gen(function* () {
				if (reader.readyState !== FileReader.LOADING) return;
				yield* RunAttachmentBrowser(file, () => reader.abort()).pipe(Effect.ignore);
			});
		});
	});

/**
 * Digests the pasted file so a re-paste is recognised before it is shown. An
 * empty digest is an accepted outcome, not a failure — `crypto.subtle` needs a
 * secure context, and losing the fast path is better than refusing the paste.
 */
const DigestSourceFile = (file: File) =>
	Effect.gen(function* () {
		const bytes = yield* Effect.tryPromise({
			catch: (cause) => new AttachmentReadError({ cause, file_name: file.name }),
			try: () => file.arrayBuffer(),
		});
		const digest = yield* Effect.tryPromise({
			catch: (cause) => new AttachmentReadError({ cause, file_name: file.name }),
			try: () => crypto.subtle.digest("SHA-256", bytes),
		});
		return [...new Uint8Array(digest)]
			.map((byte) => byte.toString(16).padStart(2, "0"))
			.join("");
	}).pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
				return "";
			}),
		),
	);

/**
 * Claims an identity, a digest, and a preview for a pasted image, so the
 * composer can show it immediately and encode behind it.
 */
export const OpenComposerImageFile = (file: File) =>
	Effect.gen(function* () {
		const snowflake_id = yield* SnowflakeId;
		const id = yield* snowflake_id.Make("attachment");
		if (!IsSupportedImageMimeType(file.type)) {
			return yield* Effect.fail(
				new AttachmentReadError({ cause: "invalid-image-data", file_name: file.name }),
			);
		}
		const source_digest = yield* DigestSourceFile(file);
		const preview_url = yield* RunAttachmentBrowser(file, () => URL.createObjectURL(file));
		return {
			content_base64: "",
			id,
			mime_type: file.type,
			name: file.name || "Image",
			preview_url,
			ready: false,
			size_bytes: 0,
			source_digest,
			source_size_bytes: file.size,
		} satisfies ComposerImageAttachment;
	});

/**
 * Encodes the image for its harness and reads the result at the FileReader
 * boundary. Encoding is best-effort — if the canvas cannot do it, the original
 * file is read instead, which every harness already accepts.
 */
export const ReadComposerImageFile = (
	attachment: ComposerImageAttachment,
	file: File,
	engine_id: string | undefined,
) =>
	Effect.gen(function* () {
		const encoded = yield* EncodeComposerImage(file, attachment.mime_type, engine_id).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					return file;
				}),
			),
		);
		const mime_type = IsSupportedImageMimeType(encoded.type)
			? encoded.type
			: attachment.mime_type;
		const reader = yield* RunAttachmentBrowser(encoded, () => new FileReader());
		const content_base64 = yield* AwaitFileReader(encoded, reader);
		return {
			...attachment,
			content_base64,
			mime_type,
			/** The name follows the bytes, so the extension never contradicts them. */
			name: encoded.name || attachment.name,
			ready: true,
			size_bytes: encoded.size,
		} satisfies ComposerImageAttachment;
	});

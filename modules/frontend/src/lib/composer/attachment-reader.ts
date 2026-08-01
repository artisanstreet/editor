import { SnowflakeId } from "@artisan/protocol";
import { Data, Effect } from "effect";
import { RunBrowserDom } from "$lib/browser/dom";
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
const AwaitFileReader = (file: File, id: ComposerImageAttachment["id"], reader: FileReader) =>
	Effect.gen(function* () {
		return yield* Effect.callback<ComposerImageAttachment, AttachmentReadError>((resume) => {
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
						const preview_url = yield* RunAttachmentBrowser(file, () =>
							URL.createObjectURL(file),
						);
						return {
							content_base64,
							id,
							mime_type: file.type,
							name: file.name || "Image",
							preview_url,
							size_bytes: file.size,
						};
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

/** Reads exactly one validated image at the FileReader browser boundary. */
export const ReadComposerImageFile = (file: File) =>
	Effect.gen(function* () {
		const snowflake_id = yield* SnowflakeId;
		const id = yield* snowflake_id.Make("attachment");
		const reader = yield* RunAttachmentBrowser(file, () => new FileReader());
		return yield* AwaitFileReader(file, id, reader);
	});

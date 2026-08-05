import { Effect } from "effect";

import { OpenComposerImageFile, ReadComposerImageFile } from "./attachment-reader";
import {
	FindDuplicateImageAttachment,
	IsDuplicateImageAttachment,
	ValidateImageAttachmentBatch,
	ValidateImageAttachmentCandidates,
	type ComposerImageAttachment,
} from "./image-attachments";

/**
 * What intake needs from the composer to place an image. The component owns the
 * editor, the attachment map, and the motion; this owns the order those happen
 * in — validate, recognise a re-paste, show, encode, settle.
 */
export interface ComposerAttachmentSurface {
	readonly Attachments: () => ReadonlyMap<string, ComposerImageAttachment>;
	/** True while the composer cannot take an image at all. */
	readonly Blocked: () => boolean;
	/** Shakes the attachments a re-paste was answered by. */
	readonly Bump: (attachment_ids: ReadonlyArray<string>) => Effect.Effect<void>;
	readonly EngineId: () => string | undefined;
	/** Adds the images to the composer and shows them where the caret is. */
	readonly Present: (
		added: ReadonlyArray<ComposerImageAttachment>,
		range?: Range,
	) => Effect.Effect<void>;
	readonly Remove: (attachment: ComposerImageAttachment) => Effect.Effect<void>;
	readonly Report: (description: string) => Effect.Effect<void>;
	readonly Revoke: (attachment: ComposerImageAttachment) => Effect.Effect<void>;
	readonly Settle: (attachment: ComposerImageAttachment) => Effect.Effect<void>;
}

export const MakeComposerAttachmentIntake = (surface: ComposerAttachmentSurface) => {
	/**
	 * Encodes one already-visible attachment and either settles it in place or
	 * takes it back out, for a batch that no longer fits or — only when no digest
	 * was available to catch it earlier — a duplicate.
	 */
	const Prepare = (attachment: ComposerImageAttachment, file: File) =>
		Effect.gen(function* () {
			const settled = yield* ReadComposerImageFile(attachment, file, surface.EngineId());
			if (!surface.Attachments().has(attachment.id)) return;
			const others = [...surface.Attachments().values()].filter(
				(item) => item.id !== attachment.id,
			);
			if (IsDuplicateImageAttachment(others, settled)) {
				yield* surface.Remove(attachment);
				yield* surface.Bump([
					others.find(
						(item) => item.ready && item.content_base64 === settled.content_base64,
					)?.id ?? "",
				]);
				return;
			}
			const fit = ValidateImageAttachmentBatch(
				others.filter((item) => item.ready),
				[settled],
			);
			if (fit !== undefined) {
				yield* surface.Remove(attachment);
				yield* surface.Report(fit);
				return;
			}
			yield* surface.Settle(settled);
		}).pipe(
			Effect.catch((cause) =>
				Effect.gen(function* () {
					yield* surface.Remove(attachment);
					yield* surface.Report(cause.message);
				}),
			),
		);

	/**
	 * Shows every pasted image at once and encodes them behind the previews. A
	 * re-paste never becomes an attachment: it is recognised from the pasted
	 * file's own digest before anything is shown, and the image already there
	 * answers for it.
	 */
	const AddFiles = (files: ReadonlyArray<File>, range?: Range) =>
		Effect.gen(function* () {
			if (surface.Blocked()) return;
			const validation = ValidateImageAttachmentCandidates(
				files.map((file) => ({ name: file.name, size_bytes: file.size, type: file.type })),
			);
			if (validation !== undefined) {
				yield* surface.Report(validation);
				return;
			}

			const opened: Array<{ attachment: ComposerImageAttachment; file: File }> = [];
			const repasted: Array<string> = [];
			yield* Effect.gen(function* () {
				for (const file of files) {
					const attachment = yield* OpenComposerImageFile(file);
					const attached = FindDuplicateImageAttachment(
						[
							...surface.Attachments().values(),
							...opened.map((item) => item.attachment),
						],
						attachment,
					);
					if (attached !== undefined) {
						repasted.push(attached.id);
						yield* surface.Revoke(attachment);
						continue;
					}
					opened.push({ attachment, file });
				}
			}).pipe(
				Effect.catch((cause) =>
					Effect.gen(function* () {
						yield* surface.Report(cause.message);
					}),
				),
			);

			if (repasted.length > 0) yield* surface.Bump(repasted);
			if (opened.length === 0) return;

			const fit = ValidateImageAttachmentBatch(
				[...surface.Attachments().values()],
				opened.map(({ attachment }) => attachment),
			);
			if (fit !== undefined) {
				for (const { attachment } of opened) yield* surface.Revoke(attachment);
				yield* surface.Report(fit);
				return;
			}

			yield* surface.Present(
				opened.map(({ attachment }) => attachment),
				range,
			);
			for (const { attachment, file } of opened) {
				yield* Prepare(attachment, file).pipe(Effect.forkScoped);
			}
		});

	return { AddFiles };
};

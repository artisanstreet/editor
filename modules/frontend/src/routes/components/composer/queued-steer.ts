import { Effect } from "effect";

import type { ComposerSubmission } from "$lib/composer/image-attachments";
import { ComposerEndRange, FocusComposerEditor, WriteComposerEditorDocument } from "./dom";

/** What the queued-steer actions borrow from the composer that hosts them. */
export interface QueuedSteerSurface {
	readonly AddFiles: (files: ReadonlyArray<File>, range?: Range) => Effect.Effect<void>;
	readonly Editor: () => HTMLDivElement | null;
	readonly Report: (title: string, description: string) => Effect.Effect<void>;
	readonly SyncEditor: () => Effect.Effect<unknown>;
	/** The steering stages' recall, already staged by generation. */
	readonly Withdraw: (generation: number) => Effect.Effect<boolean, { readonly message: string }>;
}

/** The one row of the queued stack an action operates on. */
export interface QueuedSteerRow {
	readonly generation: number;
	readonly submission: ComposerSubmission;
}

/**
 * The two things a queued steer's author may still do — discard it, or take it
 * back into the editor — built over the stages' recall. Each action names the
 * row it acts on, so any steer in the stack can be recalled, not only the
 * newest one.
 */
export const MakeQueuedSteerActions = (surface: QueuedSteerSurface) => {
	/** Runs one requested recall; a refusal reports and leaves the steer to proceed. */
	const TryWithdraw = (generation: number) =>
		surface.Withdraw(generation).pipe(
			Effect.catch((error) =>
				Effect.gen(function* () {
					yield* surface.Report("Could not discard the queued message", error.message);
					return false;
				}),
			),
		);
	/**
	 * Gives a recalled steer back to its author as composed: text into the
	 * editor, images re-attached from their encoded bytes through the ordinary
	 * intake. While a send is still in flight the intake declines additions, so
	 * an edit caught in that window restores its text alone.
	 */
	const Restore = (submission: ComposerSubmission) =>
		Effect.gen(function* () {
			const editor = surface.Editor();
			yield* WriteComposerEditorDocument(editor, submission.text, [], new Map());
			yield* surface.SyncEditor();
			if (submission.attachments.length > 0) {
				const files = submission.attachments.map(
					(part) =>
						new File(
							[
								Uint8Array.from(
									globalThis.atob(part.content_base64),
									(character) => character.codePointAt(0) ?? 0,
								),
							],
							part.name,
							{ type: part.mime_type },
						),
				);
				yield* surface.AddFiles(files, yield* ComposerEndRange(editor));
			}
			yield* FocusComposerEditor(editor);
		});
	return {
		Discard: (row: QueuedSteerRow) =>
			Effect.gen(function* () {
				yield* TryWithdraw(row.generation);
			}),
		/** Recall and hand back: the queued text returns to the editor for another pass. */
		Edit: (row: QueuedSteerRow) =>
			Effect.gen(function* () {
				if (yield* TryWithdraw(row.generation)) {
					yield* Restore(row.submission);
				}
			}),
		TryWithdraw,
	};
};

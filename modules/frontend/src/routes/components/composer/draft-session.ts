import { Effect, Option } from "effect";

import {
	ComposerDraftStore,
	SelectComposerDraftAttachmentsToRelease,
	type ComposerDraft,
} from "../../../lib/composer/draft-store";
import type { ComposerImageAttachment } from "../../../lib/composer/image-attachments";
import { ReadComposerEditorDocument, WriteComposerEditorDocument } from "./dom";

/**
 * The composer's connection to the session draft store: mirrors the live
 * document out after every change, gives it back to the next instance, and
 * settles attachment object-URL ownership when an instance dies. Attachment
 * records are shared with the store by reference — the store itself never
 * revokes a preview URL; this session decides what outlives the component.
 */
export const MakeComposerDraftSession = (options: {
	readonly draft_key: string | undefined;
	readonly Attachments: () => ReadonlyMap<string, ComposerImageAttachment>;
	readonly DraftText: () => string;
	readonly Editor: () => HTMLDivElement | null;
	/** Applies a revived draft to component state once its markers are in the DOM. */
	readonly Restored: (
		text: string,
		attachments: ReadonlyMap<string, ComposerImageAttachment>,
	) => Effect.Effect<void>;
	readonly Revoke: (attachment: ComposerImageAttachment) => Effect.Effect<void>;
}) =>
	Effect.gen(function* () {
		const store = yield* ComposerDraftStore;
		const { draft_key } = options;
		/** Keeps later editor rebinds from clobbering what the user typed since. */
		let restored = false;

		const Persist = Effect.gen(function* () {
			if (draft_key === undefined) return;
			const editor_document = yield* ReadComposerEditorDocument(
				options.Editor(),
				options.DraftText(),
			);
			const written = yield* store.Write(draft_key, {
				attachments: [...options.Attachments().values()],
				text: editor_document.text,
				tokens: editor_document.tokens,
			});
			/**
			 * Store eviction transfers ownership back through this live composer
			 * boundary. Do not revoke an attachment still owned by the just-written
			 * draft, and collapse duplicate historical references to one release.
			 */
			for (const attachment of SelectComposerDraftAttachmentsToRelease(
				written.evicted,
				new Set(options.Attachments().keys()),
			)) {
				yield* options.Revoke(attachment);
			}
		});

		const Restore = (target: HTMLDivElement | null) =>
			Effect.gen(function* () {
				if (target === null || restored || draft_key === undefined) return;
				restored = true;
				const stored = yield* store.Read(draft_key);
				if (Option.isNone(stored)) return;
				/** An attachment still encoding when the previous instance died lost its encode fiber for good. */
				for (const attachment of stored.value.attachments) {
					if (!attachment.ready) yield* options.Revoke(attachment);
				}
				const revived = new Map(
					stored.value.attachments
						.filter((attachment) => attachment.ready)
						.map((attachment) => [attachment.id, attachment] as const),
				);
				yield* WriteComposerEditorDocument(
					target,
					stored.value.text,
					stored.value.tokens,
					revived,
				);
				yield* options.Restored(stored.value.text, revived);
				yield* Persist;
			});

		/**
		 * Unmount hands attachment ownership to the store: preview URLs still
		 * referenced by the stored draft must outlive the instance so the next
		 * one can show them again. Anything the store did not retain is released.
		 */
		const ReleaseUnretained = Effect.gen(function* () {
			const stored =
				draft_key === undefined
					? Option.none<ComposerDraft>()
					: yield* store.Read(draft_key);
			const retained = new Set(
				Option.match(stored, {
					onNone: () => [] as ReadonlyArray<string>,
					onSome: (retained_draft) =>
						retained_draft.attachments.map((attachment) => attachment.id),
				}),
			);
			for (const attachment of options.Attachments().values()) {
				if (!retained.has(attachment.id)) yield* options.Revoke(attachment);
			}
		});

		const Clear = Effect.gen(function* () {
			if (draft_key !== undefined) yield* store.Clear(draft_key);
		});

		return { Clear, Persist, ReleaseUnretained, Restore };
	});

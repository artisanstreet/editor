import { Context, Effect, Layer, Option, Ref } from "effect";

import type { ComposerImageAttachment } from "./image-attachments";

/** One inline image marker's place in the draft text. */
export interface ComposerDraftToken {
	readonly id: string;
	readonly position: number;
}

/**
 * Everything the composer would lose with its instance: the typed text, the
 * attachments, and where their markers sit in the text. Attachment records are
 * shared by reference with the live composer, preview object URLs included —
 * the store never revokes them; the composer that clears or restores a draft
 * settles their ownership.
 */
export interface ComposerDraft {
	readonly attachments: ReadonlyArray<ComposerImageAttachment>;
	readonly text: string;
	readonly tokens: ReadonlyArray<ComposerDraftToken>;
}

/**
 * Retains the unsent composer document per thread across component remounts.
 * Thread switching recreates the composer via a route `{#key}`, so without
 * this store every switch silently discarded whatever was typed or attached.
 * Drafts are session memory only — nothing durable exists until the message
 * is actually sent.
 */
export class ComposerDraftStore extends Context.Service<
	ComposerDraftStore,
	{
		readonly Clear: (draft_key: string) => Effect.Effect<void>;
		readonly Read: (draft_key: string) => Effect.Effect<Option.Option<ComposerDraft>>;
		readonly Write: (draft_key: string, draft: ComposerDraft) => Effect.Effect<void>;
	}
>()("Artisan/ComposerDraftStore") {}

export const ComposerDraftStoreLive = Layer.effect(
	ComposerDraftStore,
	Effect.gen(function* () {
		const drafts = yield* Ref.make<ReadonlyMap<string, ComposerDraft>>(new Map());

		const Clear = (draft_key: string) =>
			Effect.gen(function* () {
				yield* Ref.update(drafts, (current) => {
					const next = new Map(current);
					next.delete(draft_key);
					return next;
				});
			});

		return ComposerDraftStore.of({
			Clear,
			Read: (draft_key) =>
				Effect.gen(function* () {
					const current = yield* Ref.get(drafts);
					return Option.fromUndefinedOr(current.get(draft_key));
				}),
			Write: (draft_key, draft) =>
				Effect.gen(function* () {
					if (draft.text.length === 0 && draft.attachments.length === 0) {
						yield* Clear(draft_key);
						return;
					}
					yield* Ref.update(drafts, (current) => new Map(current).set(draft_key, draft));
				}),
		});
	}),
);

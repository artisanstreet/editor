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

/** The session retains at most this many inactive drafts when their bytes permit it. */
export const MaximumRetainedComposerDrafts = 6;

/**
 * The estimate includes the UTF-16 strings V8 retains and the original bytes
 * held behind each preview object URL. The just-written draft is always
 * protected even if unusual text or image data exceeds this by itself.
 */
export const MaximumRetainedComposerDraftBytes = 64 * 1024 * 1024;

/**
 * Conservative retained-memory estimate for a draft. Base64 and text are
 * UTF-16 strings, while `source_size_bytes` accounts for the original Blob
 * held alive by its preview object URL until the owner revokes it.
 */
export const EstimateComposerDraftRetainedBytes = (draft: ComposerDraft): number =>
	draft.text.length * 2 +
	draft.tokens.reduce((total, token) => total + token.id.length * 2 + 24, 0) +
	draft.attachments.reduce(
		(total, attachment) =>
			total +
			attachment.content_base64.length * 2 +
			attachment.id.length * 2 +
			attachment.mime_type.length * 2 +
			attachment.name.length * 2 +
			attachment.preview_url.length * 2 +
			attachment.source_digest.length * 2 +
			attachment.source_size_bytes +
			128,
		0,
	);

export interface ComposerDraftWriteResult {
	readonly evicted: ReadonlyArray<ComposerDraft>;
}

/** Selects each evicted preview exactly once without releasing a currently active attachment. */
export const SelectComposerDraftAttachmentsToRelease = (
	evicted: ReadonlyArray<ComposerDraft>,
	active_attachment_ids: ReadonlySet<string>,
): ReadonlyArray<ComposerImageAttachment> => {
	const selected: Array<ComposerImageAttachment> = [];
	const selected_ids = new Set<string>();
	for (const draft of evicted) {
		for (const attachment of draft.attachments) {
			if (active_attachment_ids.has(attachment.id) || selected_ids.has(attachment.id))
				continue;
			selected_ids.add(attachment.id);
			selected.push(attachment);
		}
	}
	return selected;
};

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
		readonly Write: (
			draft_key: string,
			draft: ComposerDraft,
		) => Effect.Effect<ComposerDraftWriteResult>;
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
					const draft = yield* Ref.modify(drafts, (current) => {
						const found = current.get(draft_key);
						if (found === undefined) return [undefined, current] as const;
						/** Reading restores or exposes a draft, so it refreshes its LRU position. */
						const next = new Map(current);
						next.delete(draft_key);
						next.set(draft_key, found);
						return [found, next] as const;
					});
					return Option.fromUndefinedOr(draft);
				}),
			Write: (draft_key, draft) =>
				Effect.gen(function* () {
					if (draft.text.length === 0 && draft.attachments.length === 0) {
						yield* Clear(draft_key);
						return { evicted: [] };
					}
					return yield* Ref.modify(drafts, (current) => {
						/** Map order is LRU; the just-written draft moves to the protected tail. */
						const next = new Map(current);
						next.delete(draft_key);
						next.set(draft_key, draft);
						let retained_bytes = [...next.values()].reduce(
							(total, retained) =>
								total + EstimateComposerDraftRetainedBytes(retained),
							0,
						);
						const evicted: Array<ComposerDraft> = [];

						while (
							next.size > MaximumRetainedComposerDrafts ||
							retained_bytes > MaximumRetainedComposerDraftBytes
						) {
							const candidate = next.entries().next().value as
								| [string, ComposerDraft]
								| undefined;
							if (candidate === undefined || candidate[0] === draft_key) break;
							next.delete(candidate[0]);
							retained_bytes -= EstimateComposerDraftRetainedBytes(candidate[1]);
							evicted.push(candidate[1]);
						}

						return [{ evicted }, next] as const;
					});
				}),
		});
	}),
);

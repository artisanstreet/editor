import { Effect } from "effect";

import type { ComposerSubmission } from "../composer/image-attachments";
import { ComposerDraftStore } from "../composer/draft-store";
import { DraftThreadController } from "./draft-thread";

/** One stable composer slot for each pre-creation surface. */
export const new_thread_draft_key = (workspace_id: string | undefined): string =>
	workspace_id === undefined ? "draft:new-thread" : `draft:${workspace_id}`;

export interface NewThreadActivation {
	readonly altKey: boolean;
	readonly button: number;
	readonly ctrlKey: boolean;
	readonly metaKey: boolean;
	readonly shiftKey: boolean;
}

/** Leaves modified link activation to the browser and handles only this surface. */
export const is_unmodified_primary_activation = (event: NewThreadActivation): boolean =>
	event.button === 0 && !event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey;

/**
 * Starts an explicitly requested draft rather than reviving an abandoned one.
 * Reset runs first so a retained first submission fails closed and keeps its
 * recovery state; only a disposable pre-creation draft may have its text cleared.
 */
export const PrepareNewThreadDraft = (draft_key: string) =>
	Effect.gen(function* () {
		const draft_thread = yield* DraftThreadController;
		const drafts = yield* ComposerDraftStore;
		yield* draft_thread.Reset(drafts.Clear(draft_key));
	});

/** Creates the durable thread, then clears its now-owned first-message draft. */
export const SubmitNewThreadDraft = (draft_key: string, submission: ComposerSubmission) =>
	Effect.gen(function* () {
		const draft_thread = yield* DraftThreadController;
		const drafts = yield* ComposerDraftStore;
		return yield* Effect.uninterruptibleMask((restore) =>
			Effect.gen(function* () {
				const created = yield* restore(draft_thread.Submit(submission));
				yield* drafts.Clear(draft_key);
				return created;
			}),
		);
	});

/** Applies a retained creation retry before releasing its composer copy. */
export const RetryNewThreadDraft = (draft_key: string) =>
	Effect.gen(function* () {
		const draft_thread = yield* DraftThreadController;
		const drafts = yield* ComposerDraftStore;
		return yield* Effect.uninterruptibleMask((restore) =>
			Effect.gen(function* () {
				const created = yield* restore(draft_thread.Retry);
				yield* drafts.Clear(draft_key);
				return created;
			}),
		);
	});

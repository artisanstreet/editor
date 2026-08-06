import { Effect, Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	ComposerDraftStore,
	ComposerDraftStoreLive,
	type ComposerDraft,
} from "../../modules/frontend/src/lib/composer/draft-store";
import type { ComposerImageAttachment } from "../../modules/frontend/src/lib/composer/image-attachments";

const MakeAttachment = (id: string, ready = true): ComposerImageAttachment => ({
	content_base64: ready ? "Zm9v" : "",
	id,
	mime_type: "image/png",
	name: `${id}.png`,
	preview_url: `blob:${id}`,
	ready,
	size_bytes: 3,
	source_digest: id,
	source_size_bytes: 3,
});

const MakeDraft = (
	text: string,
	attachments: ReadonlyArray<ComposerImageAttachment> = [],
): ComposerDraft => ({
	attachments,
	text,
	tokens: attachments.map((attachment, index) => ({ id: attachment.id, position: index })),
});

const RunStore = <A>(program: Effect.Effect<A, never, ComposerDraftStore>) =>
	Effect.runPromise(program.pipe(Effect.provide(ComposerDraftStoreLive)));

describe("composer draft store", () => {
	it("retains one draft per key and returns it intact", async () => {
		const draft = MakeDraft("hello", [MakeAttachment("image-1")]);
		const restored = await RunStore(
			Effect.gen(function* () {
				const store = yield* ComposerDraftStore;
				yield* store.Write("thread-a", draft);
				return yield* store.Read("thread-a");
			}),
		);

		expect(Option.getOrThrow(restored)).toEqual(draft);
	});

	it("keeps drafts for different keys independent", async () => {
		const [first, second] = await RunStore(
			Effect.gen(function* () {
				const store = yield* ComposerDraftStore;
				yield* store.Write("thread-a", MakeDraft("for a"));
				yield* store.Write("thread-b", MakeDraft("for b"));
				yield* store.Clear("thread-a");
				return [yield* store.Read("thread-a"), yield* store.Read("thread-b")] as const;
			}),
		);

		expect(Option.isNone(first)).toBe(true);
		expect(Option.getOrThrow(second).text).toBe("for b");
	});

	it("treats an empty document as no draft at all", async () => {
		const restored = await RunStore(
			Effect.gen(function* () {
				const store = yield* ComposerDraftStore;
				yield* store.Write("thread-a", MakeDraft("typed then deleted"));
				yield* store.Write("thread-a", MakeDraft(""));
				return yield* store.Read("thread-a");
			}),
		);

		expect(Option.isNone(restored)).toBe(true);
	});

	it("keeps an attachments-only draft even with no text", async () => {
		const draft = MakeDraft("", [MakeAttachment("image-1")]);
		const restored = await RunStore(
			Effect.gen(function* () {
				const store = yield* ComposerDraftStore;
				yield* store.Write("thread-a", draft);
				return yield* store.Read("thread-a");
			}),
		);

		expect(Option.getOrThrow(restored).attachments).toHaveLength(1);
	});
});

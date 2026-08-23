import { Effect, Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	ComposerDraftStore,
	ComposerDraftStoreLive,
	MaximumRetainedComposerDraftBytes,
	MaximumRetainedComposerDrafts,
	SelectComposerDraftAttachmentsToRelease,
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

	it("moves a complete draft between route keys without copying attachment ownership", async () => {
		const draft = MakeDraft("carry me", [MakeAttachment("image-1")]);
		const result = await RunStore(
			Effect.gen(function* () {
				const store = yield* ComposerDraftStore;
				yield* store.Write("thread-a", draft);
				const move = yield* store.Move("thread-a", "draft:project-b");
				return {
					from: yield* store.Read("thread-a"),
					move,
					retained: yield* store.RetainedAttachmentIds(new Set(["image-1"])),
					to: yield* store.Read("draft:project-b"),
				};
			}),
		);

		expect(result.move).toEqual({ moved: true, orphaned: [] });
		expect(Option.isNone(result.from)).toBe(true);
		expect(Option.getOrThrow(result.to)).toEqual(draft);
		expect(result.retained).toEqual(new Set(["image-1"]));
	});

	it("reports only attachment URLs orphaned by a destination replacement", async () => {
		const replaced = MakeAttachment("replaced");
		const shared = MakeAttachment("shared");
		const move = await RunStore(
			Effect.gen(function* () {
				const store = yield* ComposerDraftStore;
				yield* store.Write("source", MakeDraft("current", [shared]));
				yield* store.Write("destination", MakeDraft("stale", [replaced, shared]));
				return yield* store.Move("source", "destination");
			}),
		);

		expect(move.moved).toBe(true);
		expect(move.orphaned.map((attachment) => attachment.id)).toEqual(["replaced"]);
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

	it("refreshes a read draft's LRU position before evicting the next inactive draft", async () => {
		const files = Array.from({ length: MaximumRetainedComposerDrafts + 2 }, (_, index) =>
			MakeDraft(`draft-${index}`),
		);
		const retained = await RunStore(
			Effect.gen(function* () {
				const store = yield* ComposerDraftStore;
				for (const [index, draft] of files.slice(0, -1).entries()) {
					yield* store.Write(`thread-${index}`, draft);
				}
				yield* store.Read("thread-1");
				yield* store.Write(`thread-${MaximumRetainedComposerDrafts + 1}`, files.at(-1)!);
				return yield* Effect.all(
					Array.from({ length: MaximumRetainedComposerDrafts + 2 }, (_, index) =>
						store.Read(`thread-${index}`),
					),
				);
			}),
		);

		expect(Option.isNone(retained[0]!)).toBe(true);
		expect(Option.isSome(retained[1]!)).toBe(true);
		expect(Option.isNone(retained[2]!)).toBe(true);
	});

	it("evicts inactive drafts when their conservative retained-byte estimate exceeds the budget", async () => {
		const large_text = "x".repeat(Math.floor(MaximumRetainedComposerDraftBytes / 2) + 1);
		const outcome = await RunStore(
			Effect.gen(function* () {
				const store = yield* ComposerDraftStore;
				const first = yield* store.Write("thread-a", MakeDraft(large_text));
				const second = yield* store.Write("thread-b", MakeDraft(large_text));
				return { first, second, retained: yield* store.Read("thread-a") };
			}),
		);

		expect(outcome.first.evicted).toEqual([]);
		expect(outcome.second.evicted.map((draft) => draft.text)).toEqual([large_text]);
		expect(Option.isNone(outcome.retained)).toBe(true);
	});

	it("protects the just-written oversized draft when no inactive draft can satisfy the budget", async () => {
		const oversized = MakeDraft("x".repeat(MaximumRetainedComposerDraftBytes + 1));
		const retained = await RunStore(
			Effect.gen(function* () {
				const store = yield* ComposerDraftStore;
				const outcome = yield* store.Write("thread-a", oversized);
				return { outcome, draft: yield* store.Read("thread-a") };
			}),
		);

		expect(retained.outcome.evicted).toEqual([]);
		expect(Option.getOrThrow(retained.draft)).toEqual(oversized);
	});

	it("selects each evicted attachment once without selecting an active attachment", () => {
		const active_attachment = MakeAttachment("active");
		const duplicate = MakeAttachment("evicted");
		const selected = SelectComposerDraftAttachmentsToRelease(
			[MakeDraft("old-a", [duplicate, active_attachment]), MakeDraft("old-b", [duplicate])],
			new Set([active_attachment.id]),
		);

		expect(selected.map((attachment) => attachment.id)).toEqual(["evicted"]);
	});
});

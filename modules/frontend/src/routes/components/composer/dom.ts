import { Effect } from "effect";

import { RunBrowserDom } from "$lib/browser/dom";
import type { ComposerDropPoint } from "$lib/composer/gesture-intake";
import type { ComposerImageAttachment } from "$lib/composer/image-attachments";

export type ComposerEditorDocument = {
	readonly text: string;
	readonly tokens: ReadonlyArray<{ readonly id: string; readonly position: number }>;
};

/** Reads text and attachment token positions from the content-editable composer. */
export const ReadComposerEditorDocument = (editor: HTMLDivElement | null, draft: string) =>
	Effect.gen(function* () {
		return yield* RunBrowserDom(() => {
			if (editor === null) return { text: draft, tokens: [] };
			const result: { text: string; tokens: Array<{ id: string; position: number }> } = {
				text: "",
				tokens: [],
			};
			const Visit = (node: Node): void => {
				if (node.nodeType === Node.TEXT_NODE) {
					result.text += node.textContent?.replaceAll("\u200B", "") ?? "";
					return;
				}
				if (!(node instanceof HTMLElement)) return;
				const attachment_id = node.dataset.attachmentId;
				if (attachment_id !== undefined) {
					result.tokens.push({ id: attachment_id, position: result.text.length });
					return;
				}
				if (node.tagName === "BR") {
					result.text += "\n";
					return;
				}
				for (const child of node.childNodes) Visit(child);
			};
			for (const node of editor.childNodes) Visit(node);
			return result;
		});
	});

export const FocusComposerRange = (range: Range) =>
	Effect.gen(function* () {
		yield* RunBrowserDom(() => {
			const selection = globalThis.getSelection();
			if (selection === null) return;
			selection.removeAllRanges();
			selection.addRange(range);
		});
	});

export const ComposerEndRange = (editor: HTMLDivElement | null) =>
	Effect.gen(function* () {
		return yield* RunBrowserDom(() => {
			const range = document.createRange();
			if (editor === null) return range;
			range.selectNodeContents(editor);
			range.collapse(false);
			return range;
		});
	});

export const ComposerSelectedRange = (editor: HTMLDivElement | null) =>
	Effect.gen(function* () {
		const selected = yield* RunBrowserDom(() => {
			const selection = globalThis.getSelection();
			if (
				editor === null ||
				selection === null ||
				selection.rangeCount === 0 ||
				!editor.contains(selection.getRangeAt(0).commonAncestorContainer)
			)
				return undefined;
			return selection.getRangeAt(0).cloneRange();
		});
		return selected ?? (yield* ComposerEndRange(editor));
	});

export const InsertComposerAttachmentMarkers = (
	next_attachments: ReadonlyArray<ComposerImageAttachment>,
	range: Range,
) =>
	Effect.gen(function* () {
		return yield* RunBrowserDom(() => {
			/** Shared with `WriteComposerEditorDocument`, which restores the same markers. */
			const make_attachment_marker = (attachment: ComposerImageAttachment) => {
				const marker = document.createElement("button");
				marker.type = "button";
				marker.contentEditable = "false";
				marker.dataset.attachmentId = attachment.id;
				marker.className = "composer-image-marker card";
				marker.setAttribute("aria-label", `View attached image ${attachment.name}`);
				marker.title = attachment.name;
				const thumbnail = document.createElement("img");
				thumbnail.alt = "";
				thumbnail.src = attachment.preview_url;
				marker.append(thumbnail);
				return marker;
			};
			range.deleteContents();
			const fragment = document.createDocumentFragment();
			for (const attachment of next_attachments) {
				fragment.append(
					make_attachment_marker(attachment),
					document.createTextNode("\u200B"),
				);
			}
			const trailing_caret = document.createTextNode("");
			fragment.append(trailing_caret);
			range.insertNode(fragment);
			const next_range = document.createRange();
			next_range.setStart(trailing_caret, 0);
			next_range.collapse(true);
			return next_range;
		});
	});

export const RemoveComposerAttachmentMarkers = (
	editor: HTMLDivElement | null,
	attachment_id: string,
) =>
	Effect.gen(function* () {
		yield* RunBrowserDom(() => {
			for (const marker of editor?.querySelectorAll<HTMLElement>("[data-attachment-id]") ??
				[]) {
				if (marker.dataset.attachmentId === attachment_id) marker.remove();
			}
		});
	});

export const FocusComposerEditor = (editor: HTMLDivElement | null) =>
	Effect.gen(function* () {
		yield* RunBrowserDom(() => editor?.focus());
	});

export const ClearComposerEditor = (editor: HTMLDivElement | null) =>
	Effect.gen(function* () {
		yield* RunBrowserDom(() => editor?.replaceChildren());
	});

/**
 * Replaces the editor content with a restored draft document: text with the
 * attachment markers reinserted at their token positions. Text nodes round-trip
 * through `ReadComposerEditorDocument` unchanged because the editor renders
 * `white-space: pre-wrap`, and marker positions round-trip because the zero
 * width spaces after markers are excluded from the read text.
 */
export const WriteComposerEditorDocument = (
	editor: HTMLDivElement | null,
	text: string,
	tokens: ReadonlyArray<{ readonly id: string; readonly position: number }>,
	attachments: ReadonlyMap<string, ComposerImageAttachment>,
) =>
	Effect.gen(function* () {
		yield* RunBrowserDom(() => {
			if (editor === null) return;
			/** The same marker `InsertComposerAttachmentMarkers` builds for a live paste. */
			const make_attachment_marker = (attachment: ComposerImageAttachment) => {
				const marker = document.createElement("button");
				marker.type = "button";
				marker.contentEditable = "false";
				marker.dataset.attachmentId = attachment.id;
				marker.className = "composer-image-marker card";
				marker.setAttribute("aria-label", `View attached image ${attachment.name}`);
				marker.title = attachment.name;
				const thumbnail = document.createElement("img");
				thumbnail.alt = "";
				thumbnail.src = attachment.preview_url;
				marker.append(thumbnail);
				return marker;
			};
			const fragment = document.createDocumentFragment();
			let cursor = 0;
			for (const token of tokens) {
				const attachment = attachments.get(token.id);
				if (attachment === undefined) continue;
				const position = Math.max(cursor, Math.min(text.length, token.position));
				if (position > cursor) {
					fragment.append(document.createTextNode(text.slice(cursor, position)));
				}
				fragment.append(make_attachment_marker(attachment), document.createTextNode("​"));
				cursor = position;
			}
			if (cursor < text.length) {
				fragment.append(document.createTextNode(text.slice(cursor)));
			}
			editor.replaceChildren(fragment);
		});
	});

export const ComposerDropRange = (editor: HTMLDivElement | null, at: ComposerDropPoint) =>
	Effect.gen(function* () {
		return yield* RunBrowserDom(() => {
			const point = document.caretRangeFromPoint?.(at.x, at.y);
			return point !== null &&
				point !== undefined &&
				editor?.contains(point.commonAncestorContainer)
				? point
				: undefined;
		});
	});

export const ComposerAttachmentIdAtEvent = (event: MouseEvent) =>
	Effect.gen(function* () {
		return yield* RunBrowserDom(() => {
			const target = event.target;
			if (!(target instanceof HTMLElement)) return undefined;
			return target.closest<HTMLElement>("[data-attachment-id]")?.dataset.attachmentId;
		});
	});

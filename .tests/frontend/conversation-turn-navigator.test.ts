import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import type { ConversationSnapshot } from "@artisan/protocol";

import {
	ActiveConversationTurn,
	ConversationTurnMarkers,
} from "../../modules/frontend/src/lib/conversation/turn-navigator";

const Read = (path: string) => readFileSync(path, "utf8");

const user_message = (id: string, text: string) =>
	({
		created_at: "2026-08-16T00:00:00.000Z",
		id,
		lifecycle: "settled",
		ordinal: 0,
		references: [],
		revision: 0,
		source_refs: [],
		text,
		turn_id: `turn_${id}`,
		type: "user_message",
		updated_at: "2026-08-16T00:00:00.000Z",
	}) as unknown as ConversationSnapshot["items"][number];

const assistant_message = (id: string) =>
	({
		...user_message(id, "reply"),
		type: "assistant_message",
	}) as ConversationSnapshot["items"][number];

describe("conversation turn navigator", () => {
	it("marks what you said and ignores what came back", () => {
		const markers = ConversationTurnMarkers([
			user_message("a", "do a timeline of the history of oslo børs"),
			assistant_message("b"),
			user_message("c", "why was it called christinia and then oslo?"),
		]);

		expect(markers).toEqual([
			{ id: "a", label: "do a timeline of the history of oslo børs" },
			{ id: "c", label: "why was it called christinia and then oslo?" },
		]);
	});

	/** A control that can only take you where you already are is noise. */
	it("offers nothing to navigate in a transcript with one turn or none", () => {
		expect(ConversationTurnMarkers([])).toEqual([]);
		expect(ConversationTurnMarkers([user_message("a", "only turn")])).toEqual([]);
	});

	it("flattens a multi-line message so a preview cannot break its row", () => {
		const [marker] = ConversationTurnMarkers([
			user_message("a", "  first line\n\n\tsecond   line  "),
			user_message("b", "another"),
		]);

		expect(marker?.label).toBe("first line second line");
	});

	it("drops a message with nothing quotable rather than listing a blank row", () => {
		expect(
			ConversationTurnMarkers([user_message("a", "   \n  "), user_message("b", "real")]),
		).toEqual([]);
	});

	/**
	 * The turn you are reading is the last one past the top edge — a turn just
	 * scrolled out of sight above still owns the position until the next
	 * arrives, which is what keeps the lit tick from flickering between two
	 * neighbours as you scroll across their boundary.
	 */
	it("reads position from the last turn that passed the top edge", () => {
		const offsets = [
			{ id: "a", top: -800 },
			{ id: "b", top: -40 },
			{ id: "c", top: 620 },
		];

		expect(ActiveConversationTurn(offsets)).toBe("b");
	});

	it("holds the first turn while the transcript is still at its top", () => {
		expect(
			ActiveConversationTurn([
				{ id: "a", top: 400 },
				{ id: "b", top: 900 },
			]),
		).toBe("a");
		expect(ActiveConversationTurn([])).toBeUndefined();
	});
});

describe("conversation turn navigator surface", () => {
	const navigator = Read(
		"modules/frontend/src/routes/components/conversation-turn-navigator.svelte",
	);
	const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

	it("rides the card's right edge and expands only on pointer or focus", () => {
		expect(navigator).toContain("absolute top-1/2 right-2");
		expect(navigator).toContain("-translate-y-1/2");
		/** The collapsed frame must not swallow clicks meant for the transcript. */
		expect(navigator).toContain("pointer-events-none");
		expect(navigator).toContain("pointer-events-auto");
	});

	/**
	 * Expanded, the map is a card — the same glass the inspector's environment
	 * card wears, at the same width, with the same pill sliding between rows.
	 * It was a bare `bg-popover` panel that expanded to whatever its longest
	 * message happened to be.
	 *
	 * The glass is a backdrop rather than a wrapper because this surface has two
	 * states and only one of them is a card: wrapping the list would paint a
	 * card behind the ticks at rest.
	 */
	it("wears the environment card's glass once expanded, and nothing at rest", () => {
		const environment = Read(
			"modules/frontend/src/routes/components/thread-environment-card.svelte",
		);

		expect(navigator).toContain("<ShaderGlassSurface");
		expect(navigator).toContain(
			"conversation-range-backdrop pointer-events-none absolute inset-0",
		);
		/** The same radius vocabulary the card facing it across the transcript uses. */
		for (const token of [
			"[--radius-gap:var(--spacing)]",
			"[--radius-surface:var(--radius-xl)]",
		]) {
			expect(navigator).toContain(token);
			expect(environment).toContain(token);
		}
		expect(navigator).toContain("group-hover:w-(--inspector-width)");
		expect(navigator).toContain("<DropdownHoverSurface");
		expect(navigator).toContain("onpointerenter={move_hover}");
	});

	it("opens and closes with the model picker's dropdown motion", () => {
		const model_picker = Read(
			"modules/frontend/src/routes/components/model-selector/view.svelte",
		);
		const dropdown_styles = Read("modules/frontend/src/lib/styles/vendor.css");

		expect(model_picker).toContain('class="t-dropdown');
		/** Both surfaces use the exact shared timings, scales, and easing. */
		for (const token of [
			"--dropdown-open-dur",
			"--dropdown-close-dur",
			"--dropdown-pre-scale",
			"--dropdown-closing-scale",
			"--dropdown-ease",
		]) {
			expect(navigator).toContain(token);
			expect(dropdown_styles).toContain(token);
		}
		expect(navigator).toContain("transform-origin: right center");
		expect(navigator).toContain("@keyframes conversation-range-in");
		expect(navigator).toContain("@media (prefers-reduced-motion: reduce)");
	});

	/** A long thread outgrows the frame; the map scrolls rather than being cut. */
	it("scrolls instead of clipping, and ends a long message in an ellipsis", () => {
		expect(navigator).toContain("overflow-y-auto");
		expect(navigator).toContain("max-h-[70%]");
		expect(navigator).toContain("min-h-0");
		expect(navigator).not.toContain("mask-r-from-85%");
		expect(navigator).toContain("truncate");
	});

	/**
	 * Collapsing is visual only. The rows stay buttons with their text in the
	 * accessibility tree at every width, so the map is never a column of unnamed
	 * controls to a screen reader or to the keyboard.
	 */
	it("keeps every turn a named control while it reads as a tick", () => {
		expect(navigator).toContain('aria-label="Conversation turns"');
		expect(navigator).toContain("<nav");
		expect(navigator).toContain("sr-only group-hover:hidden");
		expect(navigator).toContain('aria-current={active ? "true" : undefined}');
		expect(navigator).toContain('aria-hidden="true"');
	});

	it("jumps to the turn and stops the tail from pulling the reader off it", () => {
		expect(workspace).toContain("<ConversationTurnNavigator");
		expect(workspace).toContain("ConversationTurnMarkers(snapshot.items)");
		expect(workspace).toContain("ActiveConversationTurn(offsets)");
		expect(workspace).toContain("const SelectTurn = (marker: ConversationTurnMarker)");
		expect(workspace).toContain("ConversationOlderGroupCountForItem(");
		expect(workspace).toContain("older_render_group_count = required_older_groups;");
		expect(workspace).toContain("yield* Effect.promise(() => tick());");
		expect(workspace).toContain('item.scrollIntoView({ behavior: "smooth", block: "start" })');
		expect(workspace).toContain("following = false;");
		/** Position is measured from the transcript, not tracked alongside it. */
		expect(workspace).toContain("SyncActiveTurn()");
		expect(workspace).toContain("data-conversation-item-id");
	});
});

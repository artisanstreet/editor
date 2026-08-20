/**
 * The draft register.
 *
 * Grouped rather than merely ordered, because the second pass is an argument
 * with the first and an argument needs both sides adjacent: each refinement
 * sits immediately after the draft it is refining, so one press of `→` is the
 * comparison. What survived review comes first; the rest of the first pass is
 * kept behind it, because a direction that was rejected is still the reason the
 * surviving one looks the way it does.
 *
 * Each thesis is one sentence, because a variant that needs a paragraph to
 * justify its layout has already lost the argument it is making.
 */

import type { Component } from "svelte";

import Attach from "./variants/attach.svelte";
import Bento from "./variants/bento.svelte";
import Board from "./variants/board.svelte";
import Console from "./variants/console.svelte";
import Deck from "./variants/deck.svelte";
import Hearth from "./variants/hearth.svelte";
import HearthFold from "./variants/hearth-fold.svelte";
import HearthRail from "./variants/hearth-rail.svelte";
import Launcher from "./variants/launcher.svelte";
import Orbit from "./variants/orbit.svelte";
import OrbitSpring from "./variants/orbit-spring.svelte";
import Rail from "./variants/rail.svelte";
import RailQuiet from "./variants/rail-quiet.svelte";
import Segmented from "./variants/segmented.svelte";
import Sentence from "./variants/sentence.svelte";
import Token from "./variants/token.svelte";

export type StartingVariant = {
	readonly component: Component;
	/** Which argument this draft is part of, shown above its name in the gallery. */
	readonly group: string;
	readonly name: string;
	readonly thesis: string;
};

export const StartingVariants: ReadonlyArray<StartingVariant> = [
	{
		component: Hearth,
		group: "Hearth",
		name: "First pass",
		thesis: "Ask for the project before offering the field, then fold the question into one line and hand the page to the work.",
	},
	{
		component: HearthFold,
		group: "Hearth",
		name: "Fold",
		thesis: "The same page, where the fold is a fold: the chosen square travels to the header line, the rest draw in behind it, and the arrow keys work.",
	},
	{
		component: Rail,
		group: "Rail",
		name: "First pass",
		thesis: "A permanent rail on the left: you are not choosing a project per thread, you are in one, the way you are in a workspace.",
	},
	{
		component: RailQuiet,
		group: "Rail",
		name: "Quiet",
		thesis: "The same rail with its manners fixed — no remount on switch, an indicator that measures rather than guesses, and tiles that say their own name.",
	},
	{
		component: Orbit,
		group: "Orbit",
		name: "First pass",
		thesis: "Recency as distance — an hour ago sits above the field, a month ago is small and out at the edge.",
	},
	{
		component: OrbitSpring,
		group: "Orbit",
		name: "Spring",
		thesis: "The same arc on physics: it leans towards the cursor, it can be dragged, flicked, or scrolled, and it says out loud which end is which.",
	},
	{
		component: HearthRail,
		group: "Both",
		name: "Hearth → Rail",
		thesis: "Hearth and Rail are one surface at two moments — the list you pick from folds into the rail you then live in, and choosing is the animation between them.",
	},
	{
		component: Token,
		group: "Also drawn",
		name: "Token",
		thesis: "Today's page, plus one chip in the composer's control row — a project is a property of the thread exactly like the model is.",
	},
	{
		component: Segmented,
		group: "Also drawn",
		name: "Segmented",
		thesis: "Every project flat across the top as a segmented control, the active pill clipped rather than tinted, so switching costs one click and no menu.",
	},
	{
		component: Launcher,
		group: "Also drawn",
		name: "Launcher",
		thesis: "One field for projects, threads, and the first message — you type, it filters, Enter commits.",
	},
	{
		component: Board,
		group: "Also drawn",
		name: "Board",
		thesis: "The picker is the dashboard — hover previews a project's repository state and open threads, click commits the composer to it.",
	},
	{
		component: Bento,
		group: "Also drawn",
		name: "Bento",
		thesis: "Unequal tiles, because a grid of identical cards claims every project matters the same and one of them is where you live.",
	},
	{
		component: Deck,
		group: "Also drawn",
		name: "Deck",
		thesis: "Projects as a physical stack: the one you are in is the card in your hand, the rest wait behind it.",
	},
	{
		component: Sentence,
		group: "Also drawn",
		name: "Sentence",
		thesis: "The whole state as one line of English, where the parts you can change are the words you can click.",
	},
	{
		component: Console,
		group: "Also drawn",
		name: "Console",
		thesis: "A fixed table and a prompt, for hands that already have a mental model for this and it is a terminal.",
	},
	{
		component: Attach,
		group: "Also drawn",
		name: "Attach",
		thesis: "The fresh install, where no project exists yet and Forge offers the repositories it can already see on disk.",
	},
];

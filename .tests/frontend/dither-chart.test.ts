import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import {
	ConversationBaseEndSpacePixels,
	ConversationEndSpaceHeight,
	ConversationFollowTolerance,
	ConversationIsFollowing,
} from "../../modules/frontend/src/lib/conversation/scroll-position";
import {
	BackingSize,
	bayer_thresholds,
	dither_cell,
	PaintColumn,
	Resample,
} from "../../modules/frontend/src/lib/components/dither/paint";

const read = (path: string) => readFileSync(resolve(path), "utf8");

/** Collects every cell the engine paints, so the texture can be asserted by value. */
const Recorder = () => {
	const painted: Array<{ alpha: number; x: number; y: number }> = [];

	return {
		painted,
		fill: (x: number, y: number, alpha: number) => painted.push({ alpha, x, y }),
	};
};

describe("ordered dither engine", () => {
	it("normalises the 4x4 Bayer matrix into 0-1 thresholds", () => {
		expect(bayer_thresholds).toHaveLength(4);
		const flat = bayer_thresholds.flat();
		expect(flat).toHaveLength(16);
		expect(Math.min(...flat)).toBeCloseTo(0.03125);
		expect(Math.max(...flat)).toBeCloseTo(0.96875);
		/** Every threshold distinct, or the ladder would skip densities. */
		expect(new Set(flat).size).toBe(16);
	});

	/**
	 * The rule the whole engine turns on: unlit cells are painted at a lower
	 * alpha of the same colour rather than left as holes. A hole shows whatever
	 * is behind the chart — a stark speck on a light surface, invisible on a
	 * dark one.
	 */
	it("paints every cell in a column rather than punching holes", () => {
		const { fill, painted } = Recorder();
		PaintColumn(fill, 3, 0, 40, {});

		const body = painted.filter((cell) => cell.y < 40);
		/** Every row covered, plus the outline and feather overpainting the top two. */
		expect(new Set(body.map((cell) => cell.y)).size).toBe(40);
		expect(body).toHaveLength(42);
		expect(body.every((cell) => cell.alpha > 0)).toBe(true);
		/** Two tiers of one colour: lit cells and the faint off tier. */
		expect(new Set(body.map((cell) => cell.alpha > 0.3)).size).toBe(2);
	});

	it("fades upward so the fill dissolves toward its own value line", () => {
		const { fill, painted } = Recorder();
		PaintColumn(fill, 0, 0, 40, {});

		const near_floor = painted.filter((cell) => cell.y >= 30 && cell.y < 40);
		const near_line = painted.filter((cell) => cell.y > 1 && cell.y < 10);
		const mean = (cells: typeof painted) =>
			cells.reduce((sum, cell) => sum + cell.alpha, 0) / cells.length;

		expect(mean(near_floor)).toBeGreaterThan(mean(near_line));
	});

	it("caps the column with a soft outline and a feather beneath it", () => {
		const { fill, painted } = Recorder();
		PaintColumn(fill, 0, 10, 40, {});

		expect(painted.at(-2)).toMatchObject({ alpha: 0.72, y: 10 });
		expect(painted.at(-1)).toMatchObject({ alpha: 0.36, y: 11 });
	});

	it("weaves the hatched variant instead of filling every cell", () => {
		const { fill, painted } = Recorder();
		PaintColumn(fill, 0, 0, 40, { variant: "hatched" });

		/** The diagonal comb drops half the cells; the outline still lands. */
		expect(painted.filter((cell) => cell.y < 40).length).toBeLessThan(40);
		expect(painted.filter((cell) => cell.y < 40).length).toBeGreaterThan(10);
	});

	it("keeps a dotted variant's real gaps", () => {
		const { fill, painted } = Recorder();
		PaintColumn(fill, 0, 0, 40, { variant: "dotted" });

		expect(painted.filter((cell) => cell.y < 40).length).toBeLessThan(40);
	});

	it("collapses a zero-depth column to a single outline pixel", () => {
		const { fill, painted } = Recorder();
		PaintColumn(fill, 7, 20, 20, {});

		expect(painted).toEqual([{ alpha: 0.72, x: 7, y: 20 }]);
	});
});

describe("resampling", () => {
	/**
	 * Linear rather than a spline: every drawn value lies between two the
	 * provider actually reported, so the window is never drawn fuller than it
	 * ever was.
	 */
	it("interpolates linearly and never overshoots the samples", () => {
		const out = Resample([0, 100, 0], 5);

		expect(out).toEqual([0, 50, 100, 50, 0]);
		expect(Math.max(...out)).toBe(100);
	});

	it("holds a single sample flat across every column", () => {
		expect(Resample([42], 4)).toEqual([42, 42, 42, 42]);
	});

	it("sizes the backing canvas at one pixel per dither cell", () => {
		expect(dither_cell).toBe(2);
		expect(BackingSize(256, 120)).toEqual({ cols: 128, rows: 60 });
		/** Clamped so a degenerate rect cannot produce a zero-sized canvas. */
		expect(BackingSize(2, 2)).toEqual({ cols: 8, rows: 8 });
	});
});

describe("context window card", () => {
	/**
	 * The card charts the reading its own heading names. Stacking the cached and
	 * uncached token totals charted neither: those come from the provider's
	 * terminal usage frame, which accumulates across every model call in a turn,
	 * so a chain of tool calls counts the same context once per call. Cache reads
	 * dominate that sum so completely that the band rendered as solid cache at
	 * every scale, an order of magnitude above the window it sat under.
	 */
	it("states the window per model and charts how full that window was", () => {
		const details = read("modules/frontend/src/routes/components/context-usage-details.sv");

		expect(details).toContain(">Context Window<");
		expect(details).toContain("The context window for");
		expect(details).toContain("has a context window of");
		/** The separator and the raw token rows are gone. */
		expect(details).not.toContain("border-t border-border");
		expect(details).not.toContain("Cached input");
		expect(details).toContain("<DitherStackedArea");
		expect(details).toContain("point.context_tokens ?? 0");
		expect(details).toContain("lower={occupancy}");
		expect(details).not.toContain("cached_input_tokens");
		/**
		 * Captioned, because an unlabelled band is read as whatever it last was:
		 * this replaced a cached-over-fresh split, and one colour goes on saying
		 * "all cache" until something says otherwise. An early window says so too
		 * rather than leaving a blank that reads as a chart that failed.
		 */
		expect(details).toContain("Context used, per turn");
		expect(details).toContain("once a second turn has been measured");
	});

	it("scopes the series to one context window rather than the whole thread", () => {
		const service = read("modules/backend/src/surfaces/service.ts");

		expect(service).toContain('eq(SurfaceItems.kind, "compaction")');
		expect(service).toMatch(
			/gt\(\s*SurfaceItems\.projection_order,\s*boundary\.projection_order,?\s*\)/,
		);
	});

	it("scales the canvas up pixelated rather than smoothing the cells", () => {
		const chart = read("modules/frontend/src/lib/components/dither/stacked-area.sv");

		expect(chart).toContain("image-rendering: pixelated");
		expect(chart).toContain("<canvas");
	});

	/**
	 * Markers stay in SVG over the canvas: painted into the backing canvas they
	 * would be dithered and pixel-snapped with everything else.
	 */
	it("marks each point and opens a reading on hover instead of a static legend", () => {
		const chart = read("modules/frontend/src/lib/components/dither/stacked-area.sv");
		const details = read("modules/frontend/src/routes/components/context-usage-details.sv");

		expect(chart).toContain("<circle");
		expect(chart).toContain("hover_index");
		expect(chart).toContain("onpointerleave");
		expect(details).toContain("{#snippet tooltip({ index })}");
		expect(details).toContain("<ShaderGlassSurface");
		/** The standing legend row is gone; the labels live in the hover reading. */
		expect(details).not.toMatch(/turns?"?\s*}?\s*<\/span>\s*<\/div>\s*<\/div>/);
		expect(details).not.toContain("Legend");
		/**
		 * The reading is reachable only because the card is a hover surface. A
		 * tooltip dismisses on leaving its trigger, so these points could be drawn
		 * and marked and still never be hovered.
		 */
		expect(read("modules/frontend/src/routes/components/context-usage-gauge.sv")).toContain(
			"LinkPreview.Root",
		);
	});
});

describe("transcript auto-follow", () => {
	it("keeps following through the slip a growing transcript opens", () => {
		/** 600px viewport, 2000px of content: 1400 is the bottom. */
		expect(ConversationIsFollowing(1400, 2000, 600)).toBe(true);
		/** Sub-pixel remainder still counts as the bottom. */
		expect(ConversationIsFollowing(1398.5, 2000, 600)).toBe(true);
		/**
		 * A streamed word lands before the correction does. Reading that gap as a
		 * reader who scrolled away is what silently ended following mid-turn.
		 */
		expect(ConversationIsFollowing(1360, 2000, 600)).toBe(true);
		/** A deliberate scroll clears the band by far, and is never dragged back. */
		expect(ConversationIsFollowing(1300, 2000, 600)).toBe(false);
		expect(ConversationIsFollowing(900, 2000, 600)).toBe(false);
	});

	it("keeps a floor so a tiny viewport never becomes unfollowable", () => {
		expect(ConversationFollowTolerance(50)).toBe(64);
		expect(ConversationFollowTolerance(1200)).toBe(72);
	});

	it("treats content shorter than the viewport as followed", () => {
		expect(ConversationIsFollowing(0, 300, 600)).toBe(true);
	});

	it("releases the anchor guard on scrollend rather than leaving it latched", () => {
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.sv");

		expect(workspace).toContain('addEventListener("scrollend"');
		expect(workspace).toContain("anchor_scroll_active = false;");
		/** A fresh submission parks the reader at the turn's top, not the bottom. */
		expect(workspace).toContain("anchor_scroll_active = true;");
		expect(workspace).toContain("!following ||");
		expect(workspace).toContain("anchor_scroll_active ||");
		/**
		 * The follow correction pins instantly. A smooth scroll retargeted on every
		 * revealed word shivers, and its intermediate positions read as a reader
		 * who scrolled away, which switched following off for the rest of the turn.
		 */
		expect(workspace).toContain('behavior: "auto",');
	});
});

describe("turn settlement", () => {
	/**
	 * A run reaching a terminal state is only announced through the projection.
	 * Without re-reading the durable work item the transcript shows the turn
	 * finished while the composer still offers to stop a run that already ended.
	 */
	it("re-reads the work item when a turn settles", () => {
		const route = read("modules/frontend/src/routes/components/thread-route.sv");

		expect(route).toContain("const PatchSettlesTurn = (patch: ConversationPatch)");
		expect(route).toContain(
			"if (applicable.some(PatchSettlesTurn)) yield* RefreshInteractionContext;",
		);
		expect(route).toContain(
			'settled_lifecycles = new Set(["completed", "failed", "cancelled"])',
		);
	});

	/**
	 * A change set summarises what a turn did; shown mid-turn it keeps growing
	 * under the reader and reads as a finished result when it is not one.
	 */
	it("holds the changes card until the turn is over", () => {
		const store = read("modules/frontend/src/lib/conversation/store.ts");

		expect(store).toContain("settled_turn_lifecycles");
		expect(store).toContain(
			"if (turn_settled && (files.length > 0 || change_sets.length > 0))",
		);
	});

	/**
	 * Start, output and completion arrive as separate frames; keying on the frame
	 * turns one command into a row per chunk, and those rows never settle — which
	 * is what left every work group shimmering.
	 */
	it("keys terminal and tool rows on the provider id, not the frame", () => {
		const activity = read("modules/backend/src/conversation/projection/activity.ts");
		const trace = read("modules/frontend/src/routes/components/conversation-trace.sv");

		expect(activity).toContain("? observation.activity_id");
		expect(activity).toContain("? observation.tool_id");
		expect(activity).toContain("observation.search_id ?? observation.observation_id");
		expect(activity).toContain("`activity:${activity_key}`");
		/** The monospace face already says shell; the prompt glyph was noise. */
		expect(trace).not.toContain("$ {PresentShellCommand");
	});

	it("settles trace shimmer with the same reconciled session authority as its header", () => {
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.sv");

		expect(workspace).toContain(
			"work_active={block.session.ended_at === undefined && session_active}",
		);
	});
});

describe("activity group header", () => {
	/**
	 * The header says what the chain has done, always. Fronting the running
	 * command rewrote the whole line on every call and read as a different
	 * subject each time; liveness is carried by the shimmer instead, which is a
	 * treatment rather than a change of what the line is about.
	 */
	it("summarises the chain rather than following its running command", () => {
		const trace = read("modules/frontend/src/routes/components/conversation-trace.sv");

		expect(trace).toContain("const GroupIsLive = (");
		expect(trace).toContain("{@const clauses = GroupClauses(segment.id, segment.items)}");
		/**
		 * One element whether or not the chain is live. Branching to a plain span
		 * on settle replaced the subtree, and a chain goes quiet between every
		 * call — so each gap remounted the clauses and replayed their entrance,
		 * leaving the label reading "Read 2 files," with nothing after it.
		 */
		expect(trace).toContain("active={live}");
		expect(trace).toContain("trace-head-label min-w-0 flex-1 truncate");
		expect(trace).not.toContain("{@render summary()}");
		expect(trace).not.toContain("const HeadLabel = (");
		/** Neither the last command nor the running one may replace the summary. */
		expect(trace).not.toContain("activities.at(-1)");
		expect(trace).not.toContain("PresentShellCommand(live");
	});

	/**
	 * A count going 3 → 4 rewrites one clause's text in place, so nothing mounts
	 * and tabular figures keep the glyph the same width — the line cannot move.
	 * Only a kind of work appearing for the first time mounts a clause, and that
	 * is the one change with an entrance.
	 */
	it("edits counts in place and animates only a newly added clause", () => {
		const trace = read("modules/frontend/src/routes/components/conversation-trace.sv");

		expect(trace).toContain("{#each clauses as clause (clause.category)}");
		expect(trace).toContain("font-variant-numeric: tabular-nums;");
		expect(trace).toContain("@keyframes trace-clause-in");
		/** `width` has no tween to `auto`; the track carries the growth instead. */
		expect(trace).toContain("grid-template-columns: 0fr;");
		expect(trace).toContain("animation: trace-clause-in var(--duration-fast)");
		/**
		 * Only a clause added to a chain already on screen animates. Animating the
		 * ones a chain arrives with mounted the header as an icon and a chevron
		 * either side of nothing for the length of its own entrance.
		 */
		expect(trace).toContain("const painted_clauses = new Map<");
		expect(trace).toContain('clause.entering ? "trace-clause-entering" : ""');
		/** Remounting the whole label on every count is what the parts replaced. */
		expect(trace).not.toContain("{#key head}");
		expect(trace).not.toContain("t-text-swap-in");
		/** Directives take an identifier straight after the colon; CSS puts a space there. */
		expect(trace).not.toMatch(/\s(?:in|out|transition):[a-z]/);
	});

	it("fades only overflowing command text, leaving its icon and chevron outside the mask", () => {
		const trace = read("modules/frontend/src/routes/components/conversation-trace.sv");

		expect(trace).toContain("trace-command-label min-w-0 flex-1 font-mono text-sm");
		expect(trace).toContain('class="trace-acc-chevron -ml-1 flex shrink-0"');
		expect(trace).toContain('class="trace-acc-head flex w-fit max-w-full');
		expect(trace).toContain("-webkit-mask-image: linear-gradient(");
		expect(trace).toContain("mask-image: linear-gradient(");
		expect(trace).toContain("to right");
		expect(trace).toContain("animation-timeline: scroll(self inline)");
		expect(trace).toContain("animation-range: 0 100%");
		expect(trace).toContain("--trace-command-fade-end: 0px");
	});

	it("carries the failed tone onto the chevron, not just the label", () => {
		const session = read("modules/frontend/src/routes/components/conversation-work-session.sv");
		const chevron = session.slice(session.indexOf("<ChevronRight"));

		expect(chevron.slice(0, 400)).toContain('is_failed ? "text-destructive" : ""');
	});
});

describe("orphaned work sessions", () => {
	/**
	 * A run can die without emitting its terminal lifecycle event — a Forge
	 * restart takes the engine process with it — leaving the session with no
	 * `ended_at` and the transcript thinking forever while the composer, which
	 * reads the durable work item, correctly shows the run as over.
	 */
	it("settles a session the durable work item says is no longer running", () => {
		const session = read("modules/frontend/src/routes/components/conversation-work-session.sv");
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.sv");

		expect(session).toContain(
			"const ended_at = $derived(item.ended_at ?? (run_active ? undefined : item.updated_at));",
		);
		expect(session).toContain("const is_working = $derived(ended_at === undefined);");
		/** The label must read the reconciled end, not the raw item field. */
		expect(session).not.toContain("FormatDuration(item.started_at, item.ended_at)");
		/** Forwarded, wherever it sits in the prop list. */
		expect(workspace).toContain("run_active={session_active}");
	});

	/**
	 * Following a stream is not a journey to animate. It fires on every revealed
	 * word, so a smooth scroll spends its whole duration being retargeted — the
	 * transcript shivers, and the off-bottom positions it emits reach the scroll
	 * handler and read as a reader who left, ending following mid-turn. There is
	 * also nothing for reduced motion to opt out of once the pin is instant.
	 */
	it("pins to the bottom instantly rather than animating each growth", () => {
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.sv");
		const follow = workspace.slice(workspace.indexOf("!following ||")).slice(0, 500);

		expect(follow).toContain("current_viewport.scrollTo({");
		expect(follow).toContain('behavior: "auto",');
		expect(follow).not.toContain('"(prefers-reduced-motion: reduce)"');
	});

	/**
	 * The pending reference is a dependency of the statement that anchors a sent
	 * turn, so clearing it re-runs that statement and interrupts it. Run inline,
	 * the anchor pass yields for a tick before it can measure and was cut every
	 * time — the re-run carries no reference and falls through to the relayout
	 * branch, which never scrolls.
	 */
	it("anchors a sent turn outside the statement its own bookkeeping restarts", () => {
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.sv");

		expect(workspace).toContain("UpdateAnchorLayout(true).pipe(Effect.forkScoped)");
		/** The clear has to follow the fork, or it cancels what it just started. */
		const anchor = workspace.slice(workspace.indexOf("anchored_user_item_id = item_id;"));
		expect(anchor.indexOf("Effect.forkScoped")).toBeLessThan(
			anchor.indexOf("pending_user_message_reference = undefined;"),
		);
		/** Resolving the same send twice must not scroll the reader twice. */
		expect(workspace).toContain("if (anchored_user_item_id === item_id) return;");
	});

	/**
	 * The scroll stays instant — its intermediate positions are what used to end
	 * following mid-turn — and the content carries the motion instead, offset by
	 * what the scroll moved and transitioned back to zero.
	 */
	it("glides a follow correction through the content rather than the scroll", () => {
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.sv");

		expect(workspace).toContain("GlideFollowCorrection(content,");
		expect(workspace).toContain('content.style.transition = "none"');
		expect(workspace).toContain("transform var(--duration-fast) var(--ease-smooth-out)");
		/** Committing the start position is what makes the return animate at all. */
		expect(workspace).toContain("void content.offsetHeight;");
		expect(workspace).toContain("delta <= 0 || reduced_motion");
	});

	/**
	 * The reserved end space and the bottom pin are two ways to decide the scroll
	 * position, and running both makes them fight once per revealed word: the pin
	 * scrolls down by the new line, then the space shrinks by that same line a
	 * frame later and the position snaps back. The space is authoritative while
	 * it is above its floor, and bottoming out is what hands the tail over.
	 */
	it("leaves the scroll position to the reserved end space until it bottoms out", () => {
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.sv");

		expect(workspace).toContain("end_space_height > ConversationBaseEndSpacePixels");
		/** The space only ever absorbs growth, so reaching the floor is one-way. */
		expect(ConversationEndSpaceHeight(600, 0, 0)).toBe(584);
		expect(ConversationEndSpaceHeight(600, -400, 300)).toBe(ConversationBaseEndSpacePixels);
	});
});

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import {
	ConversationBaseEndSpacePixels,
	ConversationEndSpaceHeight,
	ConversationFollowTolerance,
	ConversationIsFollowing,
} from "../../modules/frontend/src/lib/conversation/scroll-position";

import { ReadStylesheets } from "./stylesheet-source";

const read = (path: string) => readFileSync(resolve(path), "utf8");

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
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.svelte");

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
	it("reuses the route's canonical conversation view instead of rebuilding it in the renderer", () => {
		const route = read("modules/frontend/src/routes/components/thread-route.svelte");
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(route).toContain("let view_state = $state.raw<ConversationViewState | undefined>()");
		expect(route).toContain("conversation_view_state={view_state}");
		expect(workspace).toContain("conversation_view_state?: ConversationViewState");
		expect(workspace).toContain("MakeParticipantConversationRenderWindow(");
		expect(workspace).not.toContain("MakeConversationViewState(snapshot)");
	});

	it("mounts a bounded recent turn window and preserves position when paging backward", () => {
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(workspace).toContain("const ConversationTurnPageSize = 24;");
		expect(workspace).toContain("older_render_group_count += ConversationTurnPageSize;");
		expect(workspace).toContain("let loading_older_turns = $state(false);");
		expect(workspace).toContain("if (loading_older_turns) return;");
		expect(workspace).toContain("disabled={loading_older_turns}");
		expect(workspace).toContain("Effect.ensuring(");
		expect(workspace).toContain("MakeParticipantConversationRenderWindow(");
		expect(workspace).toContain(
			"{#each visible_render_groups as render_group (render_group.segment_id)}",
		);
		expect(workspace).toContain("Show earlier turns ({hidden_render_group_count})");
		expect(workspace).toContain(
			"current_viewport.scrollTop += current_viewport.scrollHeight - previous_scroll_height",
		);
	});

	/**
	 * A run reaching a terminal state is only announced through the projection.
	 * Without re-reading the durable work item the transcript shows the turn
	 * finished while the composer still offers to stop a run that already ended.
	 */
	it("re-reads the work item when a turn settles", () => {
		const route = read("modules/frontend/src/routes/components/thread-route.svelte");

		expect(route).toContain("const PatchSettlesTurn = (patch: ConversationPatch)");
		expect(route).toContain(
			"if (applicable.some(PatchSettlesTurn)) yield* RefreshInteractionContext;",
		);
		/**
		 * `interrupted` settles too: nothing further arrives on its own, so the
		 * work item must be re-read rather than left mid-turn until a resume that
		 * may never come.
		 */
		expect(route).toContain(
			'settled_lifecycles = new Set(["completed", "failed", "interrupted", "cancelled"])',
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
		const trace = read("modules/frontend/src/routes/components/conversation-trace.svelte");

		expect(activity).toContain("? observation.activity_id");
		expect(activity).toContain("? observation.tool_id");
		expect(activity).toContain("observation.search_id ?? observation.observation_id");
		expect(activity).toContain("`activity:${activity_key}`");
		/** The monospace face already says shell; the prompt glyph was noise. */
		expect(trace).not.toContain("$ {PresentShellCommand");
	});

	/**
	 * The nested trace's shimmer is the same liveness question the header
	 * answers, so it reads the same settled fact. It named a deleted
	 * `session_authority` binding for a while, which is not a stale value but an
	 * undeclared identifier: rendering the details of a live work session threw
	 * a ReferenceError, took the transcript down with it, and left the composer
	 * holding a run that had already ended.
	 */
	it("settles trace shimmer on the same session status as its header", () => {
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(workspace).toContain(
			"{@const session_settled = work_session_is_settled(block.session.status)}",
		);
		expect(workspace).toContain("work_active={!session_settled &&");
		expect(workspace).toContain("block.session.ended_at === undefined}");
		expect(workspace).not.toContain("session_authority");
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
		const trace = read("modules/frontend/src/routes/components/conversation-trace.svelte");

		expect(trace).toContain("{@const clauses = GroupClauses(segment.id, segment.items)}");
		/**
		 * The shimmer belongs to the clause whose work is live, not the chain. A
		 * group-wide sweep meant a backgrounded subagent kept "Ran a command,
		 * talked to Maja" shimmering as one sentence, implying the settled command
		 * was also still running.
		 */
		expect(trace).toContain("live: work_active && members.some(conversation_activity_is_live)");
		expect(trace).toContain("active={clause.live}");
		expect(trace).not.toContain("const GroupIsLive = (");
		/**
		 * One element whether or not its clause is live. Branching to a plain span
		 * on settle replaced the subtree, and a chain goes quiet between every
		 * call — so each gap remounted the clauses and replayed their entrance,
		 * leaving the label reading "Read 2 files," with nothing after it.
		 */
		expect(trace).toContain("min-w-0 flex-1 truncate text-inherit tabular-nums");
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
		const trace = read("modules/frontend/src/routes/components/conversation-trace.svelte");
		const styles = ReadStylesheets();

		expect(trace).toContain("{#each clauses as clause (clause.category)}");
		expect(trace).toContain("truncate text-inherit tabular-nums");
		expect(styles).toContain("@keyframes trace-clause-in");
		/** `width` has no tween to `auto`; the track carries the growth instead. */
		expect(styles).toContain("grid-template-columns: 0fr;");
		expect(styles).toContain("animation: trace-clause-in var(--duration-fast)");
		/**
		 * Only a clause added to a chain already on screen animates. Animating the
		 * ones a chain arrives with mounted the header as an icon and a chevron
		 * either side of nothing for the length of its own entrance.
		 */
		expect(trace).toContain("const painted_clauses = new Map<");
		expect(trace).toContain('data-entering={clause.entering ? "true" : undefined}');
		/** Remounting the whole label on every count is what the parts replaced. */
		expect(trace).not.toContain("{#key head}");
		expect(trace).not.toContain("t-text-swap-in");
		/** Directives take an identifier straight after the colon; CSS puts a space there. */
		expect(trace).not.toMatch(/\s(?:in|out|transition):[a-z]/);
	});

	it("retires an outgoing odometer glyph after its roll, even beneath shimmer text", () => {
		const styles = ReadStylesheets();
		const outgoing = styles.slice(
			styles.indexOf('& .trace-count-char[data-outgoing="true"]'),
			styles.indexOf("\n\t}", styles.indexOf('& .trace-count-char[data-outgoing="true"]')),
		);
		const exit = styles.slice(
			styles.indexOf("@keyframes trace-count-out"),
			styles.indexOf("\n}", styles.indexOf("@keyframes trace-count-out")),
		);

		/** The exit keeps its glyph visible for the roll, then makes its filled state non-painting. */
		expect(outgoing).toContain("visibility: hidden;");
		expect(exit).toContain("visibility: visible;");
		expect(exit).toContain("to {\n\t\tvisibility: hidden;");
	});

	/** A started handoff must not make a target-only claim until its source is durable. */
	it("delegates pending handoff wording to the pure transition presentation", () => {
		const status = read("modules/frontend/src/routes/components/conversation-status.svelte");

		expect(status).toContain("model_transition_presentation(item.state, item.source_model_id)");
		expect(status).toContain('{#if presentation !== "pending_source"}');
		expect(status).toContain('{:else if presentation === "target_only"}');
	});

	it("fades only overflowing command text, leaving its icon and chevron outside the mask", () => {
		const trace = read("modules/frontend/src/routes/components/conversation-trace.svelte");
		const styles = ReadStylesheets();

		expect(trace).toContain("trace-command-label min-w-0 flex-1 font-mono text-sm");
		expect(trace).toContain("group-data-[open=true]/trace-acc:rotate-90");
		expect(trace).toContain('class="t-acc group/trace-acc flex flex-col"');
		/**
		 * A live work-session is itself an open disclosure. Its state must not
		 * cascade into a nested command/tool disclosure and make that group look
		 * permanently open regardless of its own button state.
		 */
		expect(styles).toContain("& > .t-acc-panel {");
		expect(styles).toContain('&[data-open="true"] > .t-acc-panel {');
		expect(styles).toContain("& > .t-acc-panel > .t-acc-panel-inner {");
		expect(styles).toContain('&[data-open="true"] > .t-acc-panel > .t-acc-panel-inner {');
		expect(styles).not.toContain('&[data-open="true"] .t-acc-panel {');
		expect(trace).toContain('class="trace-acc-head flex w-fit max-w-full');
		expect(styles).toContain("-webkit-mask-image: linear-gradient(");
		expect(styles).toContain("mask-image: linear-gradient(");
		expect(styles).toContain("to right");
		expect(styles).toContain("animation-timeline: scroll(self inline)");
		expect(styles).toContain("animation-range: 0 100%");
		expect(styles).toContain("--trace-command-fade-end: 0px");
	});

	it("carries the failed tone onto the chevron, not just the label", () => {
		const session = read(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		const chevron = session.slice(session.indexOf("<ChevronRight"));

		expect(chevron.slice(0, 400)).toContain('is_failed ? "text-destructive" : ""');
	});
});

describe("orphaned work sessions", () => {
	/**
	 * A run can die without emitting its terminal lifecycle event — a Forge
	 * restart takes the engine process with it. That used to be repaired in the
	 * renderer by settling against the separately fetched durable work item,
	 * which put the header's liveness on a different transport from the
	 * transcript it describes: when nothing refreshed that item, a finished run
	 * left the header thinking while the thread list lit its attention dot.
	 *
	 * Settlement now reads the session's own projected status, which arrives in
	 * sequence with the transcript, and the orphan is closed where it belongs —
	 * startup recovery journals the interruption and it lands as a patch.
	 */
	it("settles a session from its own projected status rather than a second source", () => {
		const session = read(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(session).toContain("work_session_settlement({");
		expect(session).toContain("ended_at: item.ended_at,");
		expect(session).toContain("status: item.status,");
		expect(session).toContain("updated_at: item.updated_at,");
		/** The raw fallback would resurrect the send-gap "Thought for 0s" flash. */
		expect(session).not.toContain("run_active ? undefined : item.updated_at");
		expect(session).toContain("const is_working = $derived(ended_at === undefined);");
		/** The label must read the reconciled end, not the raw item field. */
		expect(session).not.toContain("FormatDuration(item.started_at, item.ended_at)");
		/**
		 * No cross-stream authority may return: the work item reaches the
		 * renderer on its own transport, so reinstating it here reinstates the
		 * disagreement.
		 */
		expect(session).not.toContain("run_authority");
		expect(session).not.toContain("presumed_failed");
		expect(workspace).not.toContain("run_authority=");
	});

	/**
	 * A host restart is not an error, and red only means anything while it is
	 * reserved for work that actually went wrong. An interrupted run therefore
	 * reads as a stop. The status names the outcome directly now, so there is no
	 * heuristic left that could mistake being killed mid-turn for a failure.
	 */
	it("presents an interrupted run as stopped rather than failed", () => {
		const session = read(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);

		expect(session).toContain(
			'const is_stopped = $derived(item.status === "cancelled" || item.status === "interrupted");',
		);
		expect(session).toContain('const is_failed = $derived(item.status === "failed");');
		/** The failed tone is what drives every destructive class on the card. */
		expect(session).toContain('${is_failed ? "text-destructive" : ""}');
		expect(session).toContain("`Stopped after ${FormatDuration(item.started_at, ended_at)}`");
	});

	/**
	 * Following a stream is not a journey to animate. It fires on every revealed
	 * word, so a smooth scroll spends its whole duration being retargeted — the
	 * transcript shivers, and the off-bottom positions it emits reach the scroll
	 * handler and read as a reader who left, ending following mid-turn. There is
	 * also nothing for reduced motion to opt out of once the pin is instant.
	 */
	it("pins to the bottom instantly rather than animating each growth", () => {
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.svelte");
		const follow = workspace.slice(workspace.indexOf("!following ||")).slice(0, 500);

		expect(follow).toContain("current_viewport.scrollTo({");
		expect(follow).toContain('behavior: "auto",');
		expect(follow).not.toContain('"(prefers-reduced-motion: reduce)"');
	});

	/**
	 * The pending reference is a dependency of the statement that anchors a sent
	 * turn, so clearing it re-runs that statement and interrupts it. Run inline,
	 * the anchor pass yields for a tick before it can measure and was cut every
	 * time. `forkScoped` was cut the same way: it forks into the reactive run's
	 * scope, which the rerun closes. Only a fork into a component-lifetime scope
	 * survives long enough to scroll.
	 */
	it("anchors a sent turn outside the statement its own bookkeeping restarts", () => {
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(workspace).toContain("const anchor_scope = yield* Scope.make();");
		expect(workspace).toContain("Scope.close(anchor_scope, Exit.void)");
		expect(workspace).toContain("UpdateAnchorLayout(true).pipe(Effect.forkIn(anchor_scope))");
		expect(workspace).not.toContain("UpdateAnchorLayout(true).pipe(Effect.forkScoped)");
		/** The clear has to follow the fork, or it cancels what it just started. */
		const anchor = workspace.slice(workspace.indexOf("anchored_user_item_id = item_id;"));
		expect(anchor.indexOf("Effect.forkIn(anchor_scope)")).toBeLessThan(
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
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.svelte");

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
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(workspace).toContain("end_space_height > ConversationBaseEndSpacePixels");
		/** The space only ever absorbs growth, so reaching the floor is one-way. */
		expect(ConversationEndSpaceHeight(600, 0, 0)).toBe(584);
		expect(ConversationEndSpaceHeight(600, -400, 300)).toBe(ConversationBaseEndSpacePixels);
	});
});

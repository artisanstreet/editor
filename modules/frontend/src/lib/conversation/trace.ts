import type { ConversationItem } from "@artisan/protocol";
import type { ConversationRenderBlock } from "./store";

export type ConversationActivityItem = Extract<ConversationItem, { type: "activity" }>;
export type ConversationDiagnosticItem = Extract<ConversationItem, { type: "native_event" }>;
export type ConversationReasoningItem = Extract<ConversationItem, { type: "reasoning_summary" }>;
export type ConversationDiagnosticSeverity = Exclude<
	ConversationDiagnosticItem["severity"],
	undefined
>;

/** Loudest first, so a reader meets what went wrong before what merely happened. */
export const conversation_diagnostic_severities: ReadonlyArray<ConversationDiagnosticSeverity> = [
	"error",
	"warning",
	"info",
];

export type ConversationTraceSegment =
	| {
			readonly id: string;
			readonly items: ReadonlyArray<ConversationActivityItem>;
			readonly type: "activity_group";
	  }
	| {
			readonly id: string;
			readonly items: ReadonlyArray<ConversationDiagnosticItem>;
			readonly severity: ConversationDiagnosticSeverity;
			readonly type: "diagnostic_group";
	  }
	| {
			readonly id: string;
			readonly item: ConversationItem;
			readonly type: "item";
	  };

export type ConversationTraceRenderBlock =
	| ConversationRenderBlock
	| {
			readonly id: string;
			readonly items: ReadonlyArray<
				ConversationActivityItem | ConversationDiagnosticItem | ConversationReasoningItem
			>;
			readonly turn_id: string;
			readonly type: "trace_group";
	  };

type ConversationTraceRenderBlockBuilder =
	| Exclude<ConversationTraceRenderBlock, { readonly type: "trace_group" }>
	| {
			readonly id: string;
			readonly items: Array<
				ConversationActivityItem | ConversationDiagnosticItem | ConversationReasoningItem
			>;
			readonly turn_id: string;
			readonly type: "trace_group";
	  };

/**
 * Coalesces trace-only timeline blocks before the component boundary.
 *
 * Steering keeps later work below the user's message by lifting it out of the
 * original work disclosure. Those items are still one contiguous tool chain;
 * rendering each block through its own trace would erase that adjacency.
 */
export const group_conversation_trace_blocks = (
	blocks: ReadonlyArray<ConversationRenderBlock>,
): ReadonlyArray<ConversationTraceRenderBlock> => {
	const grouped: Array<ConversationTraceRenderBlockBuilder> = [];

	for (const block of blocks) {
		if (
			block.type !== "item" ||
			(block.item.type !== "activity" &&
				block.item.type !== "native_event" &&
				block.item.type !== "reasoning_summary")
		) {
			grouped.push(block);
			continue;
		}

		const previous = grouped.at(-1);
		if (previous?.type === "trace_group" && previous.turn_id === block.turn_id) {
			previous.items.push(block.item);
			continue;
		}

		grouped.push({
			id: JSON.stringify(["trace", block.id]),
			items: [block.item],
			turn_id: block.turn_id,
			type: "trace_group",
		});
	}

	return grouped;
};

const summary_headline = /^\*\*(.+?)\*\*/u;
const first_sentence = /^.*?[.!?](?=\s|$)/su;

/**
 * Reduces one reasoning item's accumulated summary to the single line the
 * thinking line says right now.
 *
 * A model's summary streams into one item per reasoning phase and keeps
 * growing — a headline, then paragraphs, then the next headline — while the
 * line is meant to say only the newest thought and let the previous one go.
 * Codex opens every section with a bold headline, and that headline is the
 * whole thought as its own client shows it, so the latest one wins over any
 * paragraph streaming under it. Claude publishes headline-less paragraphs, so
 * the newest paragraph's first sentence stands in. Whitespace collapses
 * because the line is one row: what does not fit is clipped, not wrapped.
 *
 * Only a finished sentence may take the line. A sentence used to grow into it
 * a word at a time, so the line rewrote itself continuously and then jumped
 * again when the terminator finally landed — an unreadable line that never
 * held still long enough to be read. While the newest paragraph is still
 * arriving the line keeps the last thought that did finish, and says nothing
 * at all only until the phase completes its first sentence.
 */
export const conversation_summary_line = (text: string): string | undefined => {
	const sections = text
		.split(/\n[ \t]*\n/u)
		.map((section) => section.trim())
		.filter((section) => section.length > 0);
	for (let index = sections.length - 1; index >= 0; index -= 1) {
		const headline = summary_headline.exec(sections[index] ?? "");
		if (headline?.[1] !== undefined) return headline[1].trim() || undefined;
	}
	for (let index = sections.length - 1; index >= 0; index -= 1) {
		const section = sections[index]?.replaceAll("**", "");
		const sentence = section === undefined ? undefined : first_sentence.exec(section)?.[0];
		if (sentence === undefined) continue;
		const line = sentence.replaceAll(/\s+/gu, " ").trim();
		if (line.length > 0) return line;
	}
	return undefined;
};

/**
 * Reads what the live thinking line should say, or nothing when the verb should
 * keep it.
 *
 * Reasoning is presentation-only progress, not durable transcript content: the
 * turn says what the model is thinking *now*, so exactly one line is ever on
 * screen and every earlier thought retires the moment it is superseded. Walk
 * the same block order the workspace renders, take the newest non-empty summary
 * of the active run provided nothing else visible has landed after it, and say
 * only its newest thought. Native diagnostics are metadata rather than
 * transcript prose, so they neither become the latest item nor cut the line
 * short.
 */
export const conversation_live_reasoning_summary = (
	blocks: ReadonlyArray<ConversationTraceRenderBlock>,
	active_run_id: string | undefined,
	run_active: boolean,
): string | undefined => {
	const text = conversation_live_reasoning_text(blocks, active_run_id, run_active);
	return text === undefined ? undefined : conversation_summary_line(text);
};

/**
 * Reads the full accumulated text of the reasoning phase the live thinking
 * line speaks for, or nothing when the verb should keep it. Same walk and same
 * eligibility as the one-line summary: the newest non-empty summary of the
 * active run, provided nothing else visible has landed after it — the line
 * then says only that phase's newest finished thought.
 */
export const conversation_live_reasoning_text = (
	blocks: ReadonlyArray<ConversationTraceRenderBlock>,
	active_run_id: string | undefined,
	run_active: boolean,
): string | undefined => {
	if (!run_active || active_run_id === undefined) return undefined;

	const display_order: Array<ConversationItem | null> = [];
	for (const block of blocks) {
		if (block.type === "trace_group") {
			display_order.push(...block.items);
			continue;
		}
		if (block.type === "item") {
			display_order.push(block.item);
			continue;
		}
		if (block.type === "work_group") {
			display_order.push(block.session, ...block.details);
			continue;
		}

		/** Changes and the turn footer are visible boundaries without source items. */
		display_order.push(null);
	}

	/**
	 * The newest summary this run has produced, whatever has landed since.
	 *
	 * This used to require the summary to be the newest visible item, which
	 * meant a tool call arriving after it took the line back to the thinking
	 * verb, and the next summary took it forward again — the same thought
	 * announced twice with an unrelated word in between. Nothing about the run
	 * changed at those moments; only what happened to be last in the list did.
	 *
	 * Run identity is what keeps this honest: a previous turn's thinking can
	 * never reappear, because only summaries belonging to the live run qualify.
	 */
	for (let index = display_order.length - 1; index >= 0; index -= 1) {
		const item = display_order[index];
		if (item === null || item === undefined) continue;
		if (item.type !== "reasoning_summary" || item.run_id !== active_run_id) continue;
		if (item.text.trim().length === 0) continue;
		return item.text;
	}

	return undefined;
};

/** One run of a summary body: prose, or a marked inline run within it. */
export interface ReasoningSummaryFragment {
	readonly code: boolean;
	readonly em?: boolean;
	readonly strike?: boolean;
	readonly strong?: boolean;
	readonly text: string;
}

const inline_code = /`([^`\n]+)`/gu;

/**
 * Block furniture a one-line rendering has no block to hang it on: headings,
 * quote marks, and list markers (including a model's occasional `*- ` double
 * mark) at a line's start. Bounded repetition instead of a loop, because a
 * quote can hold a list.
 */
const leading_block_marks = /^[ \t]*(?:(?:#{1,6}|>|[-*+]{1,2}|\d{1,3}[.)])[ \t]+)+/u;

const StripBlockMarks = (text: string): string =>
	text
		.split("\n")
		.map((line) => line.replace(leading_block_marks, ""))
		.join("\n");

type EmphasisFlag = "em" | "strike" | "strong";

/** Longest marks first, so `**` is never read as two `*`. */
const emphasis_marks: ReadonlyArray<{ readonly flag: EmphasisFlag; readonly mark: string }> = [
	{ flag: "strong", mark: "**" },
	{ flag: "strong", mark: "__" },
	{ flag: "strike", mark: "~~" },
	{ flag: "em", mark: "*" },
	{ flag: "em", mark: "_" },
];

const word_character = /[\p{L}\p{N}]/u;

/**
 * Underscores open and close only at word edges — `inspection_types` is an
 * identifier, not an emphasis — while asterisks follow the lighter rule that
 * an opener touches its word and a closer is touched by one, which is what
 * keeps `2 * 3` prose.
 */
const OpensEmphasis = (text: string, index: number, mark: string): boolean => {
	const next = text[index + mark.length];
	if (next === undefined || /\s/u.test(next)) return false;
	if (!mark.startsWith("_")) return true;
	const previous = text[index - 1];
	return previous === undefined || !word_character.test(previous);
};

const ClosesEmphasis = (text: string, index: number, mark: string): boolean => {
	const previous = text[index - 1];
	if (previous === undefined || /\s/u.test(previous)) return false;
	if (!mark.startsWith("_")) return true;
	const next = text[index + mark.length];
	return next === undefined || !word_character.test(next);
};

const FindEmphasisClose = (text: string, from: number, mark: string): number => {
	/** From one past the opener: an empty emphasis is two literal marks, not a pair. */
	for (let index = from + 1; index <= text.length - mark.length; index += 1) {
		if (text.startsWith(mark, index) && ClosesEmphasis(text, index, mark)) return index;
	}
	return -1;
};

type EmphasisTone = { readonly em?: boolean; readonly strike?: boolean; readonly strong?: boolean };

/**
 * Splits one prose run on its matched emphasis pairs. Only pairs count: a
 * mark whose partner never arrives stays literal, because hiding it would
 * bet on a closer a streaming sentence may never send, and prose full of
 * arithmetic asterisks must survive unstyled.
 */
const EmphasisFragments = (
	text: string,
	tone: EmphasisTone,
): ReadonlyArray<ReasoningSummaryFragment> => {
	for (let index = 0; index < text.length; index += 1) {
		for (const { flag, mark } of emphasis_marks) {
			if (!text.startsWith(mark, index)) continue;
			if (!OpensEmphasis(text, index, mark)) continue;
			const close = FindEmphasisClose(text, index + mark.length, mark);
			if (close === -1) continue;
			const inner = text.slice(index + mark.length, close);
			return [
				...(index > 0 ? [{ code: false, ...tone, text: text.slice(0, index) }] : []),
				...EmphasisFragments(inner, { ...tone, [flag]: true }),
				...EmphasisFragments(text.slice(close + mark.length), tone),
			];
		}
	}
	return text.length > 0 ? [{ code: false, ...tone, text }] : [];
};

/**
 * Splits one line of model prose into plain, emphasised, and code runs, so a
 * model naming a file or a symbol has it set in the face it meant — instead
 * of prose wearing stray `**` and backtick marks — while block furniture a
 * single line cannot honour is stripped rather than shown.
 *
 * A span whose closing backtick has not arrived yet is treated as code from
 * its opening mark. Rendering it literally instead would show a bare backtick
 * for as long as the sentence takes to finish and then reflow the line when it
 * closed — the same churn the one-line summary was just taught to avoid.
 */
export const conversation_summary_fragments = (
	raw: string,
): ReadonlyArray<ReasoningSummaryFragment> => {
	const text = StripBlockMarks(raw);
	const fragments: Array<ReasoningSummaryFragment> = [];
	let consumed = 0;
	const PushProse = (prose: string) => {
		if (prose.length > 0) fragments.push(...EmphasisFragments(prose, {}));
	};
	for (const match of text.matchAll(inline_code)) {
		const start = match.index;
		const body = match[1];
		if (body === undefined) continue;
		if (start > consumed) PushProse(text.slice(consumed, start));
		fragments.push({ code: true, text: body });
		consumed = start + match[0].length;
	}

	const tail = text.slice(consumed);
	const opened = tail.indexOf("`");
	if (opened === -1) {
		PushProse(tail);
		return fragments;
	}
	if (opened > 0) PushProse(tail.slice(0, opened));
	const arriving = tail.slice(opened + 1);
	if (arriving.length > 0) fragments.push({ code: true, text: arriving });
	return fragments;
};

/**
 * Removes reasoning before segment grouping. The thinking line says the one
 * summary that still matters, so the trace carries none of them; dropping them
 * here also lets activities on either side of a summary form one honest chain.
 */
export const strip_conversation_trace_reasoning = (
	items: ReadonlyArray<ConversationItem>,
): ReadonlyArray<ConversationItem> => items.filter((item) => item.type !== "reasoning_summary");

/**
 * Activity groups are assembled privately before becoming readonly trace
 * segments. Keeping their array mutable during one projection avoids copying
 * the entire chain for every streamed activity update.
 */
type ConversationTraceActivityGroupBuilder = {
	readonly id: string;
	readonly items: Array<ConversationActivityItem>;
	readonly type: "activity_group";
};

type ConversationTraceSegmentBuilder =
	| ConversationTraceActivityGroupBuilder
	| Exclude<ConversationTraceSegment, { readonly type: "activity_group" }>;

/**
 * Whether a text item has anything for the reader to see.
 *
 * A streamed item exists from its first delta, so between that delta and the
 * first character it is a segment that renders nothing: an invisible gap in the
 * trace, and — because any segment ends the run of activities around it — a seam
 * that split one tool chain into two.
 */
const item_renders_nothing = (item: ConversationItem): boolean =>
	(item.type === "assistant_message" || item.type === "reasoning_summary") &&
	item.text.trim().length === 0;

/**
 * Builds one deterministic work trace. Diagnostics collapse into one disclosure
 * per severity — failures, then warnings, then quiet diagnostics — at their
 * first observed position.
 *
 * `failure_visible` overrides the diagnostics preference: when the surrounding
 * work failed, its diagnostics are the explanation and must never be silenced
 * by a developer toggle.
 *
 * Activities chain while they are adjacent, and anything the agent actually said
 * starts a new one. Splitting had looked over-eager only because an item that
 * rendered nothing still counted as something between them; with those gone, a
 * seam in the trace means the agent genuinely spoke mid-run, and collapsing work
 * from either side of that into one header would claim a continuity the run did
 * not have.
 */
export const make_conversation_trace_segments = (
	items: ReadonlyArray<ConversationItem>,
	diagnostics_enabled: boolean,
	failure_visible = false,
): ReadonlyArray<ConversationTraceSegment> => {
	const diagnostics_visible = diagnostics_enabled || failure_visible;
	const diagnostics_by_severity = diagnostics_visible
		? {
				error: [] as Array<ConversationDiagnosticItem>,
				info: [] as Array<ConversationDiagnosticItem>,
				warning: [] as Array<ConversationDiagnosticItem>,
			}
		: undefined;

	/** Do not retain or classify hidden native diagnostics at all. */
	if (diagnostics_by_severity !== undefined) {
		for (const item of items) {
			if (
				item.type === "native_event" &&
				item.severity !== undefined &&
				(diagnostics_enabled ||
					(failure_visible && item.severity === "error" && item.error !== undefined))
			) {
				diagnostics_by_severity[item.severity].push(item);
			}
		}
	}

	const segments: Array<ConversationTraceSegmentBuilder> = [];
	let diagnostics_inserted = false;

	for (const item of items) {
		if (item.type === "native_event") {
			if (diagnostics_visible && !diagnostics_inserted) {
				for (const severity of conversation_diagnostic_severities) {
					const grouped = diagnostics_by_severity?.[severity] ?? [];
					if (grouped.length === 0) continue;
					segments.push({
						id: `diagnostics:${severity}`,
						items: grouped,
						severity,
						type: "diagnostic_group",
					});
				}
				diagnostics_inserted = true;
			}
			continue;
		}
		if (item_renders_nothing(item)) {
			continue;
		}
		if (item.type !== "activity") {
			segments.push({ id: item.id, item, type: "item" });
			continue;
		}

		const previous = segments.at(-1);
		if (previous?.type === "activity_group") {
			previous.items.push(item);
			continue;
		}

		segments.push({
			id: `activities:${item.id}`,
			items: [item],
			type: "activity_group",
		});
	}

	return segments;
};

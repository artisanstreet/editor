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
			readonly items: ReadonlyArray<ConversationReasoningItem>;
			readonly type: "reasoning_group";
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

/**
 * Finds the one reasoning run that is still useful at the transcript tail.
 *
 * Reasoning is presentation-only progress, not durable transcript content. Walk
 * the same block order the workspace renders and retain only consecutive,
 * non-empty summaries at the visible tail of the active run. Native diagnostics
 * are metadata rather than transcript prose, so they neither become the latest
 * item nor split an otherwise consecutive reasoning run.
 */
export const conversation_latest_reasoning_item_ids = (
	blocks: ReadonlyArray<ConversationTraceRenderBlock>,
	active_run_id: string | undefined,
	run_active: boolean,
): ReadonlySet<string> => {
	const visible_ids = new Set<string>();
	if (!run_active || active_run_id === undefined) return visible_ids;

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

	for (let index = display_order.length - 1; index >= 0; index -= 1) {
		const item = display_order[index];
		if (item === null || item === undefined) break;
		if (item.type === "native_event") continue;
		if (
			(item.type === "assistant_message" || item.type === "reasoning_summary") &&
			item.text.trim().length === 0
		)
			continue;
		if (item.type !== "reasoning_summary" || item.run_id !== active_run_id) break;
		visible_ids.add(item.id);
	}

	return visible_ids;
};

/**
 * Removes transcript-noise reasoning before segment grouping. Filtering here
 * lets activities on either side of a retired summary form one honest chain.
 */
export const filter_conversation_trace_reasoning = (
	items: ReadonlyArray<ConversationItem>,
	visible_reasoning_item_ids: ReadonlySet<string>,
): ReadonlyArray<ConversationItem> =>
	items.filter(
		(item) => item.type !== "reasoning_summary" || visible_reasoning_item_ids.has(item.id),
	);

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

type ConversationTraceReasoningGroupBuilder = {
	readonly id: string;
	readonly items: Array<ConversationReasoningItem>;
	readonly type: "reasoning_group";
};

type ConversationTraceSegmentBuilder =
	| ConversationTraceActivityGroupBuilder
	| ConversationTraceReasoningGroupBuilder
	| Exclude<ConversationTraceSegment, { readonly type: "activity_group" | "reasoning_group" }>;

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
 * Builds one deterministic work trace. Diagnostics never decide whether reasoning
 * is visible and collapse into one disclosure per severity — failures, then
 * warnings, then quiet diagnostics — at their first observed position.
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
		if (item.type === "reasoning_summary") {
			const previous = segments.at(-1);
			if (previous?.type === "reasoning_group") {
				previous.items.push(item);
				continue;
			}
			segments.push({
				id: `reasoning:${item.id}`,
				items: [item],
				type: "reasoning_group",
			});
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

import type { ConversationItem } from "@artisan/protocol";

export type ConversationActivityItem = Extract<ConversationItem, { type: "activity" }>;
export type ConversationDiagnosticItem = Extract<ConversationItem, { type: "native_event" }>;
export type ConversationDiagnosticSeverity = ConversationDiagnosticItem["severity"];

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

/**
 * Which reasoning summaries the trace shows.
 *
 * Completion is the wrong retirement signal on its own: a provider closes the
 * reasoning item when the assistant message carrying it ends, which is mid-run
 * whenever tool calls follow, so keying on it deleted thinking the reader was
 * still reading while the same run kept working. The owning work session is the
 * honest boundary — reasoning stands for as long as that work runs, and the
 * whole set retires together once it settles.
 */
const reasoning_is_visible = (
	item: ConversationItem,
	work_active: boolean,
): item is Extract<ConversationItem, { type: "reasoning_summary" }> =>
	item.type === "reasoning_summary" &&
	(work_active || item.lifecycle === "active" || item.lifecycle === "streaming");

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
 * `work_active` is the owning session's liveness, which alone decides whether
 * already-settled reasoning still belongs on screen.
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
	work_active = false,
): ReadonlyArray<ConversationTraceSegment> => {
	const diagnostics_visible = diagnostics_enabled || failure_visible;
	const diagnostics = items.filter(
		(item): item is ConversationDiagnosticItem => item.type === "native_event",
	);
	const concrete_items = items.filter(
		(item) =>
			item.type !== "reasoning_summary" &&
			item.type !== "native_event" &&
			!item_renders_nothing(item),
	);
	/**
	 * Reasoning stays visible alongside the concrete work of its own run, so
	 * streamed thinking never vanishes mid-run; a settled run retires it.
	 */
	const visible_item_ids = new Set(
		[
			...concrete_items,
			...items.filter(
				(item) => reasoning_is_visible(item, work_active) && !item_renders_nothing(item),
			),
		].map((item) => item.id),
	);
	const segments: Array<ConversationTraceSegment> = [];
	let diagnostics_inserted = false;

	for (const item of items) {
		if (item.type === "native_event") {
			if (diagnostics_visible && !diagnostics_inserted) {
				for (const severity of conversation_diagnostic_severities) {
					const grouped = diagnostics.filter(
						(diagnostic) => diagnostic.severity === severity,
					);
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
		if (!visible_item_ids.has(item.id)) continue;
		if (item.type !== "activity") {
			segments.push({ id: item.id, item, type: "item" });
			continue;
		}

		const previous = segments.at(-1);
		if (previous?.type === "activity_group") {
			segments[segments.length - 1] = {
				...previous,
				items: [...previous.items, item],
			};
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

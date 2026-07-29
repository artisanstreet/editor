import type { ConversationItem } from "@artisan/protocol";

export type ConversationActivityItem = Extract<ConversationItem, { type: "activity" }>;
export type ConversationDiagnosticItem = Extract<ConversationItem, { type: "native_event" }>;

export type ConversationTraceSegment =
	| {
			readonly id: string;
			readonly items: ReadonlyArray<ConversationActivityItem>;
			readonly type: "activity_group";
	  }
	| {
			readonly id: string;
			readonly items: ReadonlyArray<ConversationDiagnosticItem>;
			readonly type: "diagnostic_group";
	  }
	| {
			readonly id: string;
			readonly item: ConversationItem;
			readonly type: "item";
	  };

const is_active_reasoning = (
	item: ConversationItem,
): item is Extract<ConversationItem, { type: "reasoning_summary" }> =>
	item.type === "reasoning_summary" &&
	(item.lifecycle === "active" || item.lifecycle === "streaming");

/**
 * Builds one deterministic work trace. Diagnostics never decide whether reasoning
 * is visible and collapse into one disclosure at their first observed position.
 *
 * `failure_visible` overrides the diagnostics preference: when the surrounding
 * work failed, its diagnostics are the explanation and must never be silenced
 * by a developer toggle.
 */
export const make_conversation_trace_segments = (
	items: ReadonlyArray<ConversationItem>,
	diagnostics_enabled: boolean,
	failure_visible = false,
): ReadonlyArray<ConversationTraceSegment> => {
	const diagnostics_visible = diagnostics_enabled || failure_visible;
	const diagnostics = items.filter(
		(item): item is ConversationDiagnosticItem => item.type === "native_event",
	);
	const concrete_items = items.filter(
		(item) => item.type !== "reasoning_summary" && item.type !== "native_event",
	);
	const visible_item_ids = new Set(
		(concrete_items.length > 0 ? concrete_items : items.filter(is_active_reasoning)).map(
			(item) => item.id,
		),
	);
	const segments: Array<ConversationTraceSegment> = [];
	let diagnostics_inserted = false;

	for (const item of items) {
		if (item.type === "native_event") {
			if (diagnostics_visible && !diagnostics_inserted) {
				segments.push({
					id: `diagnostics:${item.id}`,
					items: diagnostics,
					type: "diagnostic_group",
				});
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

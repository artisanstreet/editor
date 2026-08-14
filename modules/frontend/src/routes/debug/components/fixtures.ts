import {
	ConversationErrorRef as ConversationErrorRefSchema,
	ConversationItem as ConversationItemSchema,
	SurfaceUsageAggregate as SurfaceUsageAggregateSchema,
	type ConversationEntityBase,
	type ConversationErrorRef,
	type ConversationItem,
	type ConversationLifecycle,
	type SurfaceUsageAggregate,
} from "@artisan/protocol";
import { Schema } from "effect";
import { MakeMockConversation } from "../../../lib/conversation/mock";

const fixture_now_ms = Date.now();
const fixture_time = (offset_ms: number): string =>
	new Date(fixture_now_ms + offset_ms).toISOString();

const fixture_entity = (
	id: string,
	ordinal: number,
	lifecycle: ConversationLifecycle = "completed",
): ConversationEntityBase => ({
	created_at: fixture_time(-180_000),
	id,
	lifecycle,
	ordinal,
	references: [],
	revision: 1,
	run_id: "run-component-gallery",
	source_refs: [{ provider: "fixture", reference: `fixture:${id}` }],
	updated_at: fixture_time(-2_000),
});

/** Keeps each mock's exact union member while still validating the wire shape. */
const define_item = <Item extends ConversationItem>(item: Item): Item => {
	Schema.decodeUnknownSync(ConversationItemSchema)(item);
	return item;
};

const define_error = <ErrorRef extends ConversationErrorRef>(error: ErrorRef): ErrorRef => {
	Schema.decodeUnknownSync(ConversationErrorRefSchema)(error);
	return error;
};

const define_usage = <Usage extends SurfaceUsageAggregate>(usage: Usage): Usage => {
	Schema.decodeUnknownSync(SurfaceUsageAggregateSchema)(usage);
	return usage;
};

const turn_id = "turn-component-gallery";

export const gallery_user_message = define_item({
	...fixture_entity("gallery-user-message", 1),
	text: "Can you make compaction feel like a new chapter inside the thread?",
	turn_id,
	type: "user_message",
});

export const gallery_image_message = define_item({
	...fixture_entity("gallery-image-message", 2),
	attachments: [
		{
			id: "gallery-artisan-mark",
			media_type: "image/png",
			name: "artisan-mark.png",
			size_bytes: 24_960,
		},
	],
	content: [
		{ text: "Use this mark as the visual reference.", type: "text" },
		{ attachment_id: "gallery-artisan-mark", type: "image" },
	],
	text: "Use this mark as the visual reference.",
	turn_id,
	type: "user_message",
});

export const gallery_assistant_message = define_item({
	...fixture_entity("gallery-assistant-message", 3),
	phase: "final",
	text: [
		"The compaction boundary now reads as a new context chapter.",
		"",
		"- The label keeps one stable node.",
		"- Shimmer stops when compaction settles.",
		"- Both rules flex without overflowing.",
		"",
		"```svelte[conversation-status.svelte]{2}",
		'<ShimmerText active={item.state === "started"}>',
		"\t{label}",
		"</ShimmerText>",
		"```",
	].join("\n"),
	turn_id,
	type: "assistant_message",
});

export const gallery_streaming_message = define_item({
	...fixture_entity("gallery-streaming-message", 4, "streaming"),
	phase: "commentary",
	text: "I’ve mapped the current thread cards. The remaining work is isolating their mock boundaries so the gallery can",
	turn_id,
	type: "assistant_message",
});

export const gallery_reasoning_summary = define_item({
	...fixture_entity("gallery-reasoning-summary", 5, "streaming"),
	text: "The gallery should render production components, not visual replicas, and mount only one ticking specimen at a time.",
	turn_id,
	type: "reasoning_summary",
});

export const gallery_active_work = define_item({
	...fixture_entity("gallery-active-work", 6, "active"),
	responded_at: fixture_time(-52_000),
	started_at: fixture_time(-67_000),
	status: "active",
	title: "Building the thread component gallery",
	turn_id,
	type: "work_session",
});

export const gallery_completed_work = define_item({
	...fixture_entity("gallery-completed-work", 7),
	ended_at: fixture_time(-32_000),
	responded_at: fixture_time(-91_000),
	started_at: fixture_time(-104_000),
	status: "completed",
	title: "Verified the component gallery",
	turn_id,
	type: "work_session",
});

export const gallery_failed_work = define_item({
	...fixture_entity("gallery-failed-work", 8, "failed"),
	ended_at: fixture_time(-24_000),
	responded_at: fixture_time(-74_000),
	started_at: fixture_time(-82_000),
	status: "failed",
	title: "Started the provider turn",
	turn_id,
	type: "work_session",
});

export const gallery_activity = define_item({
	...fixture_entity("gallery-activity-read", 9),
	detail: "modules/frontend/src/routes/components/conversation-status.svelte",
	kind: "read",
	label: "Read conversation status",
	status: "completed",
	turn_id,
	type: "activity",
});

export const gallery_active_activity = define_item({
	...fixture_entity("gallery-activity-terminal", 10, "active"),
	detail: "pnpm run validate:frontend",
	kind: "terminal_activity",
	label: "Terminal",
	status: "active",
	turn_id,
	type: "activity",
});

export const gallery_search_activity = define_item({
	...fixture_entity("gallery-activity-search", 11),
	detail: "Svelte component gallery accessibility patterns",
	kind: "search",
	label: "Search",
	status: "completed",
	turn_id,
	type: "activity",
});

export const gallery_tool_activity = define_item({
	...fixture_entity("gallery-activity-tool", 12),
	detail: "Inspected the component in the browser",
	kind: "tool",
	label: "Browser screenshot",
	status: "completed",
	turn_id,
	type: "activity",
});

export const gallery_error_ref = define_error({
	affected_model_id: "claude-fable-5",
	code: "AE-PROVIDER-201",
	detail: "The provider rejected the turn because the weekly Fable window is exhausted.",
	limit_id: "seven_day:fable",
	limit_label: "Fable",
	limit_scope: "model",
	provider_code: "rate_limit_error",
	resets_at: fixture_time(8_040_000),
});

export const gallery_error_event = define_item({
	...fixture_entity("gallery-provider-error-event", 13, "failed"),
	error: gallery_error_ref,
	severity: "error",
	summary: "The provider usage limit was reached before the response completed.",
	turn_id,
	type: "native_event",
});

export const gallery_trace_items = [
	gallery_reasoning_summary,
	gallery_activity,
	gallery_search_activity,
	gallery_tool_activity,
	gallery_active_activity,
] as const;

export const gallery_failed_trace_items = [gallery_activity, gallery_error_event] as const;

export const gallery_change_set = define_item({
	...fixture_entity("gallery-change-set", 14),
	file_count: 3,
	file_ids: ["gallery-file-route", "gallery-file-fixtures", "gallery-file-test"],
	state: "applied",
	summary: "Added the thread component gallery",
	turn_id,
	type: "change_set",
});

export const gallery_file_changes = [
	define_item({
		...fixture_entity("gallery-file-route", 15),
		change_set_id: gallery_change_set.id,
		diff: { additions: 184, deletions: 0, kind: "known" },
		operation: "created",
		path: "C:\\Users\\sander\\Desktop\\artisan-editor\\modules\\frontend\\src\\routes\\debug\\components\\+page.svelte",
		turn_id,
		type: "file_change",
	}),
	define_item({
		...fixture_entity("gallery-file-fixtures", 16),
		change_set_id: gallery_change_set.id,
		diff: { additions: 312, deletions: 0, kind: "known" },
		operation: "created",
		path: "C:\\Users\\sander\\Desktop\\artisan-editor\\modules\\frontend\\src\\routes\\debug\\components\\fixtures.ts",
		turn_id,
		type: "file_change",
	}),
	define_item({
		...fixture_entity("gallery-file-test", 17),
		change_set_id: gallery_change_set.id,
		diff: { additions: 76, deletions: 2, kind: "known" },
		operation: "modified",
		path: "C:\\Users\\sander\\Desktop\\artisan-editor\\.tests\\frontend\\components-gallery.test.ts",
		turn_id,
		type: "file_change",
	}),
] as const;

export const gallery_approval = define_item({
	...fixture_entity("gallery-command-approval", 18, "waiting"),
	interaction_id: "approval-component-gallery",
	prompt: "Allow this command?",
	request: {
		command: "pnpm run validate:frontend",
		cwd: "C:\\Users\\sander\\Desktop\\artisan-editor",
		kind: "command",
		reason: "Runs the frontend formatter, lint, build, and focused test gate.",
	},
	requested_at: fixture_time(-12_000),
	state: "requested",
	turn_id,
	type: "approval",
});

export const gallery_question = define_item({
	...fixture_entity("gallery-question", 19, "waiting"),
	interaction_id: "question-component-gallery",
	prompt: "Should the gallery wrap from the last component back to the first?",
	requested_at: fixture_time(-9_000),
	state: "requested",
	turn_id,
	type: "question",
});

const gallery_usage_interruption = {
	affected_model_id: "claude-fable-5",
	alternatives: [
		{
			display_name: "Claude Opus 5",
			engine_id: "claude",
			model_id: "claude-opus-5",
			verified_at: fixture_time(-3_000),
		},
	],
	auto_continue: true,
	created_at: fixture_time(-18_000),
	interruption_id: "usage-component-gallery",
	limit_id: "seven_day:fable",
	limit_label: "Fable",
	limit_scope: "model",
	provider_code: "rate_limit_error",
	resets_at: fixture_time(8_040_000),
	resume_not_before: fixture_time(8_040_000),
	revision: 2,
	source_agent_id: "agent-root",
	source_engine_id: "claude",
	source_model_id: "claude-fable-5",
	source_run_id: "run-component-gallery",
	state: "scheduled",
	thread_id: "thread-component-gallery",
	updated_at: fixture_time(-3_000),
} as const;

export const gallery_usage_limit = define_item({
	...fixture_entity("gallery-usage-limit", 20, "waiting"),
	interruption: gallery_usage_interruption,
	turn_id,
	type: "usage_interruption",
});

export const gallery_usage_continued = define_item({
	...fixture_entity("gallery-usage-continued", 21),
	interruption: {
		...gallery_usage_interruption,
		continuation_command_id: "command-component-gallery",
		continued_at: fixture_time(-2_000),
		revision: 4,
		state: "continued",
		target_engine_id: "claude",
		target_model_id: "claude-opus-5",
		target_run_id: "run-component-gallery-continuation",
	},
	turn_id,
	type: "usage_interruption",
});

export const gallery_compacting = define_item({
	...fixture_entity("gallery-compacting", 22, "active"),
	portability: "portable",
	state: "started",
	summary: "Preparing a smaller context for the next model turn.",
	turn_id,
	type: "compaction",
});

export const gallery_compacted = define_item({
	...fixture_entity("gallery-compacted", 23),
	portability: "portable",
	state: "completed",
	summary: "The next model turn starts from a compacted context.",
	turn_id,
	type: "compaction",
});

export const gallery_model_handoff = define_item({
	...fixture_entity("gallery-model-handoff", 24, "active"),
	continuation: "native",
	source_engine_id: "claude",
	source_model_id: "claude-fable-5",
	state: "started",
	target_engine_id: "claude",
	target_model_id: "claude-opus-5",
	turn_id,
	type: "model_transition",
});

export const gallery_context_usage = define_usage({
	cached_input_tokens: 18_420,
	context_origin: {
		engine_id: "claude",
		model_id: "claude-fable-5",
		run_id: "run-component-gallery",
	},
	context_tokens: 148_700,
	context_window_tokens: 200_000,
	input_tokens: 132_840,
	output_tokens: 15_860,
	scope: "run",
	scope_id: "run-component-gallery",
});

export const gallery_turn_settled_at = fixture_time(-92_000);
export const gallery_thread_snapshot = MakeMockConversation("thread-component-gallery");

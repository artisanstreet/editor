import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import { ConversationItem } from "@artisan/protocol";
import { conversation_progress_phase } from "../../modules/frontend/src/lib/conversation/activity-status";
import {
	group_conversation_trace_blocks,
	make_conversation_trace_segments,
} from "../../modules/frontend/src/lib/conversation/trace";

const ReadSource = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

const base = {
	created_at: "2026-07-26T00:00:00.000Z",
	lifecycle: "completed",
	ordinal: 1,
	references: [],
	revision: 0,
	run_id: "run_1",
	source_refs: [],
	turn_id: "run:run_1",
	updated_at: "2026-07-26T00:00:00.000Z",
};

const item = (value: unknown) => Schema.decodeUnknownSync(ConversationItem)(value);

describe("conversation trace", () => {
	it("restores the historical aggregate header without changing category-aware rows", () => {
		const trace = ReadSource(
			"modules/frontend/src/routes/components/conversation-trace.svelte",
		);
		const workspace = ReadSource(
			"modules/frontend/src/routes/components/thread-workspace.svelte",
		);

		expect(trace).toContain("GetConversationActivityCategoryLabel(activity_category)");
		expect(trace).toContain('class="shrink-0 text-foreground"');
		expect(trace).toContain("const CategoryIcon");
		expect(trace).toContain(
			'category === "command" || category === "test" || category === "typecheck"',
		);
		expect(trace).toContain('if (category === "file_read") return FileText');
		expect(trace).toContain('if (category === "file_edit") return FilePencil');
		expect(trace).toContain('if (category === "file_delete") return FileX');
		expect(trace).toContain('if (category === "file_search") return FileSearch');
		expect(trace).toContain('if (category === "web_search") return WorldSearch');
		expect(trace).toContain("const GroupIcon");
		expect(trace).toContain("return only === undefined ? ListDetails : CategoryIcon(only[0])");
		expect(trace).toContain('<HeadIcon class="size-4 shrink-0" aria-hidden="true" />');
		expect(trace).toContain(
			"index === 0 ? clause.charAt(0).toUpperCase() + clause.slice(1) : `, ${clause}`",
		);
		expect(trace).toContain(
			"text-muted-foreground transition-colors duration-150 hover:text-foreground group-data-[open=true]/trace-acc:text-foreground motion-reduce:transition-none",
		);
		const group_header = trace.match(/<ShimmerText[\s\S]*?<\/ShimmerText>/u)?.[0];
		expect(group_header).toBeDefined();
		expect(group_header).not.toContain("GetConversationActivityCategoryLabel");
		expect(group_header).not.toContain('class="text-foreground"');
		expect(group_header).not.toContain('class="text-muted-foreground"');
		expect(workspace).toContain("group_conversation_trace_blocks(");
		expect(workspace).toContain(
			"conversation_live_reasoning_summary(render_blocks, active_run_id, run_active)",
		);
		expect(workspace).not.toContain("<ConversationTrace items={[block.item]}");
	});

	it("keeps contiguous post-steer trace blocks in one tool chain", () => {
		const steering = item({
			...base,
			id: "message_steer",
			ordinal: 1,
			text: "Keep checking the remaining commands.",
			turn_id: "turn:user:steer",
			type: "user_message",
		});
		const activity = (id: string, ordinal: number) =>
			item({
				...base,
				id,
				kind: "terminal_activity",
				label: "Ran a command",
				ordinal,
				status: "completed",
				type: "activity",
			});
		const assistant = item({
			...base,
			id: "assistant_boundary",
			ordinal: 4,
			phase: "commentary",
			text: "I checked the first result.",
			type: "assistant_message",
		});
		const as_block = (entry: ConversationItem) => ({
			id: entry.id,
			item: entry,
			turn_id: entry.turn_id,
			type: "item" as const,
		});
		const grouped = group_conversation_trace_blocks([
			as_block(steering),
			as_block(activity("activity_1", 2)),
			as_block(activity("activity_2", 3)),
			as_block(assistant),
			as_block(activity("activity_3", 5)),
			as_block(activity("activity_4", 6)),
		]);

		expect(grouped).toEqual([
			expect.objectContaining({ id: "message_steer", type: "item" }),
			expect.objectContaining({
				items: [
					expect.objectContaining({ id: "activity_1" }),
					expect.objectContaining({ id: "activity_2" }),
				],
				type: "trace_group",
			}),
			expect.objectContaining({ id: "assistant_boundary", type: "item" }),
			expect.objectContaining({
				items: [
					expect.objectContaining({ id: "activity_3" }),
					expect.objectContaining({ id: "activity_4" }),
				],
				type: "trace_group",
			}),
		]);
	});

	it("routes post-steer reasoning through the same trace group as adjacent work", () => {
		const activity = (id: string, ordinal: number) =>
			item({
				...base,
				id,
				kind: "terminal_activity",
				label: "Ran a command",
				ordinal,
				status: "completed",
				type: "activity",
			});
		const reasoning = item({
			...base,
			id: "reasoning_after_steer",
			ordinal: 2,
			text: "Checking the steered request",
			type: "reasoning_summary",
		});
		const as_block = (entry: ConversationItem) => ({
			id: entry.id,
			item: entry,
			turn_id: entry.turn_id,
			type: "item" as const,
		});

		expect(
			group_conversation_trace_blocks([
				as_block(activity("activity_before_reasoning", 1)),
				as_block(reasoning),
				as_block(activity("activity_after_reasoning", 3)),
			]),
		).toEqual([
			expect.objectContaining({
				items: [
					expect.objectContaining({ id: "activity_before_reasoning" }),
					expect.objectContaining({ id: "reasoning_after_steer" }),
					expect.objectContaining({ id: "activity_after_reasoning" }),
				],
				type: "trace_group",
			}),
		]);
	});

	it("places a visual-only rail beside expanded activity children", () => {
		const trace = ReadSource(
			"modules/frontend/src/routes/components/conversation-trace.svelte",
		);

		expect(trace).toContain('class="relative flex flex-col gap-1 pl-6"');
		expect(trace).toContain("pointer-events-none absolute inset-y-0 left-0 w-4");
		expect(trace).toContain("after:left-1/2 after:w-[2px]");
		expect(trace).toContain('aria-hidden="true"');
		expect(trace).not.toContain('aria-label="Collapse activity group"');
		expect(trace).not.toContain('title="Collapse activity group"');
		expect(trace).not.toContain("hover:after:bg-foreground/50");
		/** The native header button remains the sole accessible disclosure control. */
		expect(trace).toContain("aria-expanded={open}");
		expect(trace).toContain("onclick={yield* ToggleGroup(segment.id)}");
	});

	it("never lets native diagnostics bypass the trace visibility policy", () => {
		const item_view = ReadSource(
			"modules/frontend/src/routes/components/conversation-item.svelte",
		);
		const status = ReadSource(
			"modules/frontend/src/routes/components/conversation-status.svelte",
		);

		expect(item_view).toContain('{:else if item.type === "native_event"}');
		expect(item_view).toContain("Native diagnostics render only through ConversationTrace");
		expect(status).not.toContain('{:else if item.type === "native_event"}');
		expect(status).not.toContain('<Badge variant="outline">Native</Badge>');
	});

	it("keeps tool-chain rows free of output hover previews", () => {
		const source = readFileSync(
			"modules/frontend/src/routes/components/conversation-trace.svelte",
			"utf8",
		);

		expect(source).not.toContain("LinkPreview");
		expect(source).not.toContain("ShaderGlassSurface");
		expect(source).not.toContain("openDelay={0}");
		expect(source).not.toContain("tabindex={activity.output");
		expect(source).not.toContain("preview_props");
	});

	it("hides diagnostics by default without suppressing what the agent said", () => {
		const segments = make_conversation_trace_segments(
			[
				item({
					...base,
					id: "diagnostic_1",
					summary: "Provider warning",
					type: "native_event",
				}),
				item({
					...base,
					id: "assistant_1",
					lifecycle: "active",
					ordinal: 2,
					phase: "commentary",
					text: "Checking the provider",
					type: "assistant_message",
				}),
			],
			false,
		);

		expect(segments).toEqual([expect.objectContaining({ id: "assistant_1", type: "item" })]);
	});

	it("leaves the trace with no reasoning surface of its own", () => {
		const trace = ReadSource(
			"modules/frontend/src/routes/components/conversation-trace.svelte",
		);
		const workspace = ReadSource(
			"modules/frontend/src/routes/components/thread-workspace.svelte",
		);

		/** The rail, its disclosure, and the whole reasoning block are gone. */
		expect(trace).not.toContain("ConversationReasoningSummary");
		expect(trace).not.toContain("reasoning_group");
		expect(trace).not.toContain("open_groups[segment.id] ?? work_active");
		expect(workspace).toContain(
			"conversation_live_reasoning_summary(render_blocks, active_run_id, run_active)",
		);
		expect(workspace).toContain("has_details={visible_details.length > 0}");
	});

	/**
	 * A streamed item exists from its first delta. Before its first character it
	 * renders nothing, and any segment ends the run of activities around it — so
	 * an invisible item both left a blank in the trace and split one continuous
	 * tool chain into two headers.
	 */
	it("gives no segment to a streamed item that has yet to render anything", () => {
		const activity = (id: string, ordinal: number) =>
			item({
				...base,
				id,
				kind: "terminal_activity",
				label: "Ran a command",
				ordinal,
				status: "completed",
				type: "activity",
			});
		const empty_reasoning = item({
			...base,
			id: "reasoning_empty",
			lifecycle: "streaming",
			ordinal: 2,
			text: "   ",
			type: "reasoning_summary",
		});

		const segments = make_conversation_trace_segments(
			[activity("activity_1", 1), empty_reasoning, activity("activity_2", 3)],
			false,
			false,
		);

		expect(segments).toEqual([
			expect.objectContaining({ id: "activities:activity_1", type: "activity_group" }),
		]);
	});

	/**
	 * Adjacency is the whole rule. Something the agent actually said between two
	 * batches is a real seam in the run, and folding work from both sides of it
	 * into one header would claim a continuity the run did not have.
	 */
	it("chains adjacent activities and lets anything the agent said start a new one", () => {
		const activity = (id: string, kind: string, ordinal: number) =>
			item({
				...base,
				id,
				kind,
				label: "Provider activity",
				ordinal,
				status: "completed",
				type: "activity",
			});
		const spoken = item({
			...base,
			id: "assistant_1",
			lifecycle: "streaming",
			ordinal: 2,
			phase: "commentary",
			text: "Deciding what to read next",
			type: "assistant_message",
		});

		const segments = make_conversation_trace_segments(
			[
				activity("activity_1", "terminal_activity", 1),
				spoken,
				activity("activity_2", "terminal_activity", 3),
				activity("activity_3", "search", 4),
			],
			false,
		);

		expect(segments.map((segment) => segment.type)).toEqual([
			"activity_group",
			"item",
			"activity_group",
		]);
		const chains = segments.flatMap((segment) =>
			segment.type === "activity_group" ? [segment.items.map((entry) => entry.id)] : [],
		);
		expect(chains).toEqual([["activity_1"], ["activity_2", "activity_3"]]);
	});

	it("projects a long adjacent activity chain without rebuilding prior groups", () => {
		const activities = Array.from({ length: 4_096 }, (_, index) =>
			item({
				...base,
				id: `activity_${index}`,
				kind: "terminal_activity",
				label: "Ran a command",
				ordinal: index + 1,
				status: "completed",
				type: "activity",
			}),
		);

		const segments = make_conversation_trace_segments(activities, false);

		expect(segments).toHaveLength(1);
		expect(segments[0]).toMatchObject({
			id: "activities:activity_0",
			type: "activity_group",
		});
		expect(segments[0]?.type === "activity_group" ? segments[0].items : []).toEqual(activities);
		const source = ReadSource("modules/frontend/src/lib/conversation/trace.ts");
		expect(source).not.toContain("items: [...previous.items, item]");
	});

	it("groups diagnostics by severity when enabled, loudest disclosure first", () => {
		const segments = make_conversation_trace_segments(
			[
				/** A row persisted before severities existed decodes as quiet info. */
				item({
					...base,
					id: "diagnostic_legacy",
					summary: "Usage update",
					type: "native_event",
				}),
				item({
					...base,
					id: "activity_1",
					kind: "terminal_activity",
					label: "Ran a command",
					ordinal: 2,
					status: "completed",
					type: "activity",
				}),
				item({
					...base,
					id: "diagnostic_warning",
					ordinal: 3,
					severity: "warning",
					summary: "Provider warning",
					type: "native_event",
				}),
				item({
					...base,
					id: "diagnostic_error",
					ordinal: 4,
					severity: "error",
					summary: "Provider failure",
					type: "native_event",
				}),
			],
			true,
		);

		expect(segments.filter((segment) => segment.type === "diagnostic_group")).toEqual([
			expect.objectContaining({
				id: "diagnostics:error",
				items: [expect.objectContaining({ id: "diagnostic_error" })],
				severity: "error",
				type: "diagnostic_group",
			}),
			expect.objectContaining({
				id: "diagnostics:warning",
				items: [expect.objectContaining({ id: "diagnostic_warning" })],
				severity: "warning",
				type: "diagnostic_group",
			}),
			expect.objectContaining({
				id: "diagnostics:info",
				items: [expect.objectContaining({ id: "diagnostic_legacy" })],
				severity: "info",
				type: "diagnostic_group",
			}),
		]);
	});

	it("keeps an unclassified historical diagnostic out of visible severity groups", () => {
		const historical_diagnostic = {
			...base,
			id: "diagnostic_unclassified",
			lifecycle: "completed",
			summary: "Persisted before diagnostic severity existed",
			type: "native_event",
		} satisfies Extract<ConversationItem, { type: "native_event" }>;

		expect(make_conversation_trace_segments([historical_diagnostic], true)).toEqual([]);
	});

	it("keeps unclassified provider stderr out of failed conversations", () => {
		const failure_diagnostic = item({
			...base,
			id: "diagnostic_failure",
			severity: "error",
			summary:
				"Engine startup failed before the native session became ready (EngineConfigurationError).",
			type: "native_event",
		});

		const hidden = make_conversation_trace_segments([failure_diagnostic], false, false);
		const surfaced = make_conversation_trace_segments([failure_diagnostic], false, true);

		expect(hidden).toEqual([]);
		expect(surfaced).toEqual([]);
		const leaked_protocol_lifecycle = item({
			...base,
			id: "diagnostic_protocol_lifecycle",
			severity: "info",
			summary: "Item reasoning started",
			type: "native_event",
		});
		expect(make_conversation_trace_segments([leaked_protocol_lifecycle], false, true)).toEqual(
			[],
		);
	});

	it("renders failed work as an unmissable failure in the workspace", () => {
		const work_session = ReadSource(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		const trace = ReadSource(
			"modules/frontend/src/routes/components/conversation-trace.svelte",
		);
		const workspace = ReadSource(
			"modules/frontend/src/routes/components/thread-workspace.svelte",
		);

		expect(work_session).toContain("`Failed after ${FormatDuration(");
		expect(work_session).toContain("`Stopped after ${FormatDuration(");
		/** An interrupted run earns the same disclosure: the reader did not ask for it. */
		expect(work_session).toMatch(
			/previous_status\s*===\s*"running"\s*&&\s*\(\s*status\s*===\s*"failed"\s*\|\|\s*status\s*===\s*"cancelled"\s*\|\|\s*status\s*===\s*"interrupted"\s*\)/,
		);
		expect(work_session).toContain(
			"if (became_unsuccessful && !user_chose_disclosure) open = true;",
		);
		expect(work_session).toContain('is_failed ? "text-destructive" : ""');
		expect(trace).toContain("make_conversation_trace_segments(");
		expect(trace).toContain("$conversation_diagnostics_enabled,");
		expect(trace).toContain('label: "Failures",');
		expect(trace).toContain('failed && segment.severity === "error"');
		expect(trace).toContain('role={alerting ? "alert" : undefined}');
		/** Cancellation is the user's own act; only failed work re-skins the trace. */
		expect(workspace).toContain("{#snippet details(session_failed: boolean)}");
		expect(workspace).toContain("failed={session_failed}");
	});

	/**
	 * The reply retires the history it came from: the trace folds itself the
	 * moment the model starts writing below it, unfolds if the model goes back
	 * to work, and never fights a disclosure the reader chose themselves.
	 */
	it("starts unfinished traces open and folds only on an observed reply phase", () => {
		const work_session = ReadSource(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		const workspace = ReadSource(
			"modules/frontend/src/routes/components/thread-workspace.svelte",
		);

		/** Mounting mid-reply preserves the live session's initially-open state. */
		expect(work_session).toContain(
			"let previous_progress_phase = untrack(() => progress_phase);",
		);
		expect(work_session).toContain("let previous_working = untrack(");
		expect(work_session).toContain("const phase_changed = phase !== previous_progress_phase;");
		expect(work_session).toContain("const settled = previous_working && !working;");
		expect(work_session).toContain('if (phase === "work" && working) open = true;');
		expect(work_session).toContain(
			'else if (phase === "reply" && (phase_changed || settled)) open = false;',
		);
		expect(work_session).toContain(
			"yield* ReconcileReplyDisclosure(progress_phase, is_working, item.status);",
		);
		expect(workspace).toContain("progress_phase={conversation_progress_phase(block.details)}");
	});

	it("reads resumed work off ordinals, not off a text lifecycle an engine may dangle", () => {
		const message = (ordinal: number, text: string) =>
			item({
				...base,
				id: `assistant_${ordinal}`,
				lifecycle: "streaming",
				ordinal,
				phase: "commentary",
				text,
				type: "assistant_message",
			});
		const running = (ordinal: number) =>
			item({
				...base,
				id: `activity_${ordinal}`,
				kind: "terminal_activity",
				label: "Ran a command",
				lifecycle: "streaming",
				ordinal,
				status: "active",
				type: "activity",
			});
		const thinking = (ordinal: number) =>
			item({
				...base,
				id: `reasoning_${ordinal}`,
				lifecycle: "streaming",
				ordinal,
				text: "Weighing the options",
				type: "reasoning_summary",
			});

		/** Prose is the newest thing: the fold stands. */
		expect(conversation_progress_phase([running(1), message(2, "So far…")])).toBe("reply");
		/** A newer activity or a newer reasoning phase both mean work resumed. */
		expect(conversation_progress_phase([message(1, "So far…"), running(2)])).toBe("work");
		expect(conversation_progress_phase([message(1, "So far…"), thinking(2)])).toBe("work");
		/** No prose yet: nothing to fold for, so nothing to resume from. */
		expect(conversation_progress_phase([running(1), thinking(2)])).toBe("work");
		expect(conversation_progress_phase([message(1, "   "), running(2)])).toBe("work");
		expect(
			conversation_progress_phase([
				message(1, "So far…"),
				item({
					...base,
					id: "reasoning_empty",
					lifecycle: "streaming",
					ordinal: 2,
					text: "   ",
					type: "reasoning_summary",
				}),
			]),
		).toBe("reply");
		expect(conversation_progress_phase([message(1, "   ")])).toBe("none");
	});
});

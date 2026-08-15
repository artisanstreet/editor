import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { ConversationItem } from "@artisan/protocol";
import { Schema } from "effect";
import { describe, expect, it } from "vitest";
import {
	conversation_latest_reasoning_item_ids,
	filter_conversation_trace_reasoning,
	group_conversation_trace_blocks,
	make_conversation_trace_segments,
} from "../../modules/frontend/src/lib/conversation/trace";

const ReadSource = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

const base = {
	created_at: "2026-08-15T00:00:00.000Z",
	lifecycle: "completed",
	ordinal: 1,
	references: [],
	revision: 0,
	run_id: "run_1",
	source_refs: [],
	turn_id: "turn:user:steer",
	updated_at: "2026-08-15T00:00:00.000Z",
};

const item = (value: unknown) => Schema.decodeUnknownSync(ConversationItem)(value);

const reasoning = (id: string, ordinal: number, text: string, run_id = "run_1") =>
	item({
		...base,
		id,
		lifecycle: "streaming",
		ordinal,
		run_id,
		text,
		type: "reasoning_summary",
	});

const activity = (id: string, ordinal: number) =>
	item({
		...base,
		id,
		kind: "tool_activity",
		label: "Read a file",
		ordinal,
		status: "completed",
		type: "activity",
	});

const as_block = (entry: ReturnType<typeof item>) => ({
	id: entry.id,
	item: entry,
	turn_id: entry.turn_id,
	type: "item" as const,
});

describe("conversation thinking trace presentation", () => {
	it("shows a lone trace item directly, but replaces the first of a multi-item trace", () => {
		const source = ReadSource(
			"modules/frontend/src/routes/components/conversation-reasoning-summary.svelte",
		);

		expect(source).toContain(
			"const single_item = $derived(items.length === 1 ? items[0] : undefined);",
		);
		expect(source).toContain("const trace_items = $derived(items.slice(1));");
		expect(source).toContain("{#if single_item !== undefined}");
		expect(source).toContain("markdown={single_item.text}");
		expect(source.match(/use:reveal/gu)).toHaveLength(3);
		expect(source).toContain("{:else if items.length > 1}");
		expect(source).toContain("{thinking_word}");
		expect(source).toContain("{#each trace_items as summary, index (summary.id)}");
		expect(source).not.toContain("{#each items as summary");
	});

	it("renders reasoning through the safe shared Comark parsing boundary", () => {
		const source = ReadSource(
			"modules/frontend/src/routes/components/conversation-reasoning-summary.svelte",
		);

		expect(source).toContain('import { Comark } from "@comark/svelte"');
		expect(source).toContain(
			'import { conversation_parse_options } from "$lib/components/markdown/parsing"',
		);
		expect(source.match(/options=\{conversation_parse_options\}/gu)).toHaveLength(2);
		expect(source).not.toMatch(/\.replace\(|\.replaceAll\(/u);
		expect(source).not.toContain("MarkdownContent");
		expect(source).not.toContain("StreamWord");
	});

	it("has no disclosure controls and keeps the rail visual-only", () => {
		const source = ReadSource(
			"modules/frontend/src/routes/components/conversation-reasoning-summary.svelte",
		);

		expect(source).not.toContain("ontoggle");
		expect(source).not.toContain("aria-expanded");
		expect(source).not.toContain("<button");
		expect(source).not.toContain("ChevronRight");
		expect(source).not.toContain("reasoning-acc");
		expect(source).toContain("pointer-events-none absolute inset-y-0 left-0 w-4");
		expect(source).toContain('aria-hidden="true"');
	});

	it("uses the texts-reveal hooks, variables, action orchestration, and motion fallback", () => {
		const source = ReadSource(
			"modules/frontend/src/routes/components/conversation-reasoning-summary.svelte",
		);

		expect(source).toContain(":global(:root)");
		for (const variable of [
			"--stagger-dur",
			"--stagger-distance",
			"--stagger-stagger",
			"--stagger-blur",
			"--stagger-ease",
		]) {
			expect(source).toContain(variable);
		}
		expect(source).toContain('class="t-stagger" use:reveal');
		expect(source).toContain("t-stagger-line");
		expect(source).toContain("transition-delay: calc(var(--stagger-stagger) * ${index + 1});");
		expect(source).toContain('node.classList.remove("is-shown")');
		expect(source).toContain("void node.offsetHeight");
		expect(source).toContain('node.classList.add("is-shown")');
		expect(source).toContain("will-change: transform, opacity, filter;");
		expect(source).toContain("@media (prefers-reduced-motion: reduce)");
		expect(source).not.toMatch(/setTimeout|Effect\./u);
	});

	it("keeps only the active run's consecutive reasoning at the visible transcript tail", () => {
		const blocks = group_conversation_trace_blocks([
			as_block(reasoning("reasoning_old", 1, "Old thought", "run_old")),
			as_block(activity("activity_boundary", 2)),
			as_block(reasoning("reasoning_2", 3, "Checking the result")),
			as_block(
				item({
					...base,
					id: "diagnostic_1",
					ordinal: 4,
					severity: "info",
					summary: "Provider metadata",
					type: "native_event",
				}),
			),
			as_block(reasoning("reasoning_blank", 5, "   ")),
			as_block(reasoning("reasoning_3", 6, "Preparing the answer")),
		]);

		expect([...conversation_latest_reasoning_item_ids(blocks, "run_1", true)].sort()).toEqual([
			"reasoning_2",
			"reasoning_3",
		]);
		expect(conversation_latest_reasoning_item_ids(blocks, "run_1", false).size).toBe(0);
	});

	it("retires reasoning once later visible transcript content arrives", () => {
		const summary = reasoning("reasoning_1", 1, "Checking the request");
		const assistant = item({
			...base,
			id: "assistant_1",
			ordinal: 2,
			phase: "commentary",
			text: "The check is complete.",
			type: "assistant_message",
		});
		const blocks = group_conversation_trace_blocks([as_block(summary), as_block(assistant)]);

		expect(conversation_latest_reasoning_item_ids(blocks, "run_1", true).size).toBe(0);
		expect(filter_conversation_trace_reasoning([summary], new Set())).toEqual([]);
	});

	it("filters retired reasoning before regrouping adjacent activities", () => {
		const first_activity = activity("activity_1", 1);
		const retired = reasoning("reasoning_1", 2, "First pass");
		const second_activity = activity("activity_2", 3);
		const latest = reasoning("reasoning_2", 4, "Second pass");
		const visible = filter_conversation_trace_reasoning(
			[first_activity, retired, second_activity, latest],
			new Set([latest.id]),
		);

		expect(make_conversation_trace_segments(visible, false)).toEqual([
			expect.objectContaining({
				items: [
					expect.objectContaining({ id: "activity_1" }),
					expect.objectContaining({ id: "activity_2" }),
				],
				type: "activity_group",
			}),
			expect.objectContaining({
				items: [expect.objectContaining({ id: "reasoning_2" })],
				type: "reasoning_group",
			}),
		]);
	});

	it("wires post-steer liveness by item run ID and removes empty settled detail", () => {
		const source = ReadSource("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(source).toContain(
			"conversation_latest_reasoning_item_ids(render_blocks, active_run_id, run_active)",
		);
		expect(source).toContain("block.items.some((item) => item.run_id === active_run_id)");
		expect(source).not.toContain(
			"work_active={run_active && block.turn_id === `run:${active_run_id}`}",
		);
		expect(source).toContain("items={visible_trace_items}");
		expect(source).toContain("items={visible_details}");
		expect(source).toContain("has_details={visible_details.length > 0}");
	});
});

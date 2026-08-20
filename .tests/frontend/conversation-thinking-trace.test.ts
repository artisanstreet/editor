import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { ConversationItem } from "@artisan/protocol";
import { Schema } from "effect";
import { describe, expect, it } from "vitest";
import {
	active_work_label_for,
	artisan_thinking_words,
} from "../../modules/frontend/src/lib/conversation/activity-status";
import {
	conversation_live_reasoning_summary,
	conversation_live_reasoning_text,
	conversation_summary_line,
	group_conversation_trace_blocks,
	make_conversation_trace_segments,
	strip_conversation_trace_reasoning,
} from "../../modules/frontend/src/lib/conversation/trace";
import { policy_reasoning_display } from "../../modules/frontend/src/lib/engine/reasoning-display";

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

const policy = (engine_id: string, model: string) => ({
	engine_id,
	model,
	permission: "supervised",
	permission_mode: "on_request" as const,
	reasoning_effort: "high" as const,
	sandbox_mode: "workspace_write" as const,
	service_tier: "standard",
	strict_clarification: false,
	web_search_enabled: false,
});

describe("live thinking line", () => {
	it("says the active run's newest summary, without its markdown emphasis", () => {
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
			as_block(reasoning("reasoning_3", 6, "**Preparing the answer**\n\nOne pass remains. ")),
		]);

		expect(conversation_live_reasoning_summary(blocks, "run_1", true)).toBe(
			"Preparing the answer",
		);
		expect(conversation_live_reasoning_summary(blocks, "run_1", false)).toBeUndefined();
		expect(conversation_live_reasoning_summary(blocks, "run_other", true)).toBeUndefined();
	});

	it("says only the newest thought, one line, replaced as the next one arrives", () => {
		/** Codex: every section opens with a headline, and the latest headline is the thought. */
		expect(conversation_summary_line("**Considering the options**")).toBe(
			"Considering the options",
		);
		expect(
			conversation_summary_line(
				"**Considering the options**\n\nTwo of them fit. One is cheaper.\n\n**Checking the cheaper one**\n\nIt reads the manif",
			),
		).toBe("Checking the cheaper one");
		/** Claude: headline-less paragraphs, so the newest paragraph's first sentence stands in. */
		const claude =
			"My three edits to wire.ts all landed correctly — the import/re-export block and both unions. Now I'm moving to the backend.\n\n" +
			"I'm deciding to get the hostname directly via node:os instead of adding a dependency, and I'm sketching a parser. For the service itself, I'm defining a HostMachinesService.";
		expect(conversation_summary_line(claude)).toBe(
			"I'm deciding to get the hostname directly via node:os instead of adding a dependency, and I'm sketching a parser.",
		);
		/**
		 * A sentence still arriving does not take the line. Letting it in rewrote
		 * the line on every streamed word and then jumped again at the full stop,
		 * so the reader never had a thought that held still long enough to read.
		 * The last finished thought keeps the line until the next one finishes.
		 */
		expect(conversation_summary_line("Done with the parser.\n\nNext I'm loo")).toBe(
			"Done with the parser.",
		);
		/** Nothing to say yet is the one case that still yields to the verb. */
		expect(conversation_summary_line("Next I'm loo")).toBeUndefined();
		expect(conversation_summary_line("Line one\nof the same paragraph.")).toBe(
			"Line one of the same paragraph.",
		);
		expect(conversation_summary_line("   \n\n  ")).toBeUndefined();

		const session = ReadSource(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		expect(session).toContain('class="trace-command-label min-w-0 truncate"');
		expect(session).not.toContain("whitespace-pre-wrap text-base leading-7");
	});

	it("falls back to the thinking verb before the model has summarized anything", () => {
		const inputs = {
			engine_name: "Claude",
			provider_responded: true,
			seed: "work:run:run_1",
			waiting_for_activity: false,
		};

		expect(artisan_thinking_words).toContain(active_work_label_for(inputs));
		expect(
			active_work_label_for({ ...inputs, reasoning_summary: "Reading the manifest" }),
		).toBe("Reading the manifest");
		/** Facts about the run outrank whatever the model last said about itself. */
		expect(
			active_work_label_for({
				...inputs,
				reasoning_summary: "Reading the manifest",
				waiting_for_activity: true,
			}),
		).toBe("Waiting");
		expect(
			active_work_label_for({
				...inputs,
				background_agent_names: ["Explore"],
				reasoning_summary: "Reading the manifest",
			}),
		).toBe("Waiting for Explore to finish…");
		expect(
			active_work_label_for({
				...inputs,
				provider_responded: false,
				reasoning_summary: "Reading the manifest",
			}),
		).toBe("Waiting for Claude to respond…");
	});

	it("never rides a thinking count on the verb, even for the engine that estimates one", () => {
		/**
		 * The engine still reports its encrypted-reasoning estimate and the
		 * protocol still carries it, but no status line spends it: "Pondering"
		 * never becomes "Pondering · 1.2k" anywhere in the render path.
		 */
		const status = ReadSource("modules/frontend/src/lib/conversation/activity-status.ts");
		const trace = ReadSource("modules/frontend/src/lib/conversation/trace.ts");
		const session = ReadSource(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		const workspace = ReadSource(
			"modules/frontend/src/routes/components/thread-workspace.svelte",
		);

		expect(status).not.toContain("thinking_tokens");
		expect(status).not.toContain("thinking_label_for");
		expect(trace).not.toContain("thinking_tokens");
		expect(session).not.toContain("thinking_tokens");
		expect(workspace).not.toContain("thinking_tokens");
	});

	it("retires the line once later visible transcript content arrives", () => {
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

		expect(conversation_live_reasoning_summary(blocks, "run_1", true)).toBeUndefined();
	});

	it("keeps reasoning out of the trace so adjacent activities stay one chain", () => {
		const items = [
			activity("activity_1", 1),
			reasoning("reasoning_1", 2, "First pass"),
			activity("activity_2", 3),
			reasoning("reasoning_2", 4, "Second pass"),
		];

		expect(
			make_conversation_trace_segments(strip_conversation_trace_reasoning(items), false),
		).toEqual([
			expect.objectContaining({
				items: [
					expect.objectContaining({ id: "activity_1" }),
					expect.objectContaining({ id: "activity_2" }),
				],
				type: "activity_group",
			}),
		]);
	});

	it("says a summary only for models that publish them", () => {
		expect(policy_reasoning_display(policy("claude", "claude-opus-5"))).toBe("summary");
		expect(policy_reasoning_display(policy("codex", "gpt-6-codex"))).toBe("summary");
		/** Open-weight models stream raw thought, which is never a public summary. */
		expect(policy_reasoning_display(policy("cursor", "kimi-k3"))).toBe("trace");
		expect(policy_reasoning_display(policy("cursor", "glm-5.2"))).toBe("trace");
		/** An unknown or absent model must not silence a line it could say. */
		expect(policy_reasoning_display(policy("claude", "some-unlisted-model"))).toBe("summary");
		expect(policy_reasoning_display(undefined)).toBe("summary");
	});

	it("renders one shimmering muted line and no reasoning block of its own", () => {
		const session = ReadSource(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		const trace = ReadSource(
			"modules/frontend/src/routes/components/conversation-trace.svelte",
		);
		const workspace = ReadSource(
			"modules/frontend/src/routes/components/thread-workspace.svelte",
		);

		expect(session).toContain("reasoning_summary,");
		expect(session).toContain('class="trace-command-label min-w-0 truncate"');
		expect(session).toContain("max-w-(--prose-body-width)");
		expect(session).toContain("{#if renders_status_line}");
		/**
		 * The summary is one line, full stop: a thought that outruns the row
		 * truncates into the trace's clipped-command fade rather than opening
		 * the phase's prose downward behind a disclosure.
		 */
		expect(session).not.toContain("thinking_expanded");
		expect(session).not.toContain("summary_sections");
		expect(session).not.toContain("reasoning_summary_text");
		/** The rail and its disclosure were their own block; there is no second line. */
		expect(trace).not.toContain("ConversationReasoningSummary");
		expect(trace).not.toContain("reasoning_group");
		expect(workspace).not.toContain("ConversationReasoningSummary");
		expect(workspace).toContain('policy_reasoning_display(policy) === "trace"');
		expect(workspace).toContain(
			"conversation_live_reasoning_summary(render_blocks, active_run_id, run_active)",
		);
		expect(workspace).toContain("block.session.run_id === active_run_id");
		expect(workspace).toContain("strip_conversation_trace_reasoning(block.details)");
		expect(workspace).toContain("has_details={visible_details.length > 0}");
	});
});

describe("full reasoning text behind the line", () => {
	it("returns the whole phase text where the line says only its newest finished thought", () => {
		const text = "First paragraph of thought.\n\nSecond thought still arriving";
		const blocks = [as_block(reasoning("summary_full", 1, text))];
		/** The phase whole, including the paragraph still landing. */
		expect(conversation_live_reasoning_text(blocks, "run_1", true)).toBe(text);
		/** The one-line status waits for that paragraph to finish its sentence. */
		expect(conversation_live_reasoning_summary(blocks, "run_1", true)).toBe(
			"First paragraph of thought.",
		);
	});
});

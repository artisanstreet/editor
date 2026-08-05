import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import { ConversationItem } from "@artisan/protocol";
import { make_conversation_trace_segments } from "../../modules/frontend/src/lib/conversation/trace";

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
	it("hides diagnostics by default without suppressing active reasoning", () => {
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
					id: "reasoning_1",
					lifecycle: "active",
					ordinal: 2,
					text: "Checking the provider",
					type: "reasoning_summary",
				}),
			],
			false,
		);

		expect(segments).toEqual([expect.objectContaining({ id: "reasoning_1", type: "item" })]);
	});

	it("keeps reasoning visible for the whole live run and retires it once work settles", () => {
		const activity = item({
			...base,
			id: "activity_1",
			kind: "tool_activity",
			label: "Read a file",
			ordinal: 2,
			status: "completed",
			type: "activity",
		});
		const reasoning = (lifecycle: string) =>
			item({
				...base,
				id: "reasoning_1",
				lifecycle,
				ordinal: 3,
				text: "Reading the skill reference",
				type: "reasoning_summary",
			});

		const streaming = make_conversation_trace_segments(
			[activity, reasoning("streaming")],
			false,
		);
		expect(streaming).toEqual([
			expect.objectContaining({ type: "activity_group" }),
			expect.objectContaining({ id: "reasoning_1", type: "item" }),
		]);

		/**
		 * A provider closes the reasoning item when the assistant message carrying
		 * it ends — mid-run whenever more tool calls follow. Thinking the reader is
		 * still reading must not vanish underneath them while the run continues.
		 */
		const completed_mid_run = make_conversation_trace_segments(
			[activity, reasoning("completed")],
			false,
			false,
			true,
		);
		expect(completed_mid_run).toEqual([
			expect.objectContaining({ type: "activity_group" }),
			expect.objectContaining({ id: "reasoning_1", type: "item" }),
		]);

		const settled = make_conversation_trace_segments([activity, reasoning("completed")], false);
		expect(settled).toEqual([expect.objectContaining({ type: "activity_group" })]);
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
			true,
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
		const reasoning = item({
			...base,
			id: "reasoning_1",
			lifecycle: "streaming",
			ordinal: 2,
			text: "Deciding what to read next",
			type: "reasoning_summary",
		});

		const segments = make_conversation_trace_segments(
			[
				activity("activity_1", "terminal_activity", 1),
				reasoning,
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

	it("always surfaces diagnostics for failed work, overriding the preference", () => {
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
		expect(surfaced).toEqual([
			expect.objectContaining({
				items: [expect.objectContaining({ id: "diagnostic_failure" })],
				severity: "error",
				type: "diagnostic_group",
			}),
		]);
	});

	it("renders failed work as an unmissable failure in the workspace", () => {
		const work_session = ReadSource(
			"modules/frontend/src/routes/components/conversation-work-session.sv",
		);
		const trace = ReadSource("modules/frontend/src/routes/components/conversation-trace.sv");
		const workspace = ReadSource("modules/frontend/src/routes/components/thread-workspace.sv");

		expect(work_session).toContain("`Failed after ${FormatDuration(");
		expect(work_session).toContain("`Cancelled after ${FormatDuration(");
		expect(work_session).toContain(
			'previous_status === "running" && (status === "failed" || status === "cancelled")',
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
		expect(workspace).toContain('failed={block.session.status === "failed"}');
	});
});

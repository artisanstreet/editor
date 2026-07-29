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

	it("keeps live reasoning visible after concrete work starts and retires it on completion", () => {
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

		const completed = make_conversation_trace_segments([activity, reasoning("completed")], false);
		expect(completed).toEqual([expect.objectContaining({ type: "activity_group" })]);
	});

	it("groups every diagnostic behind one disclosure when enabled", () => {
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
					id: "activity_1",
					kind: "terminal_activity",
					label: "Ran a command",
					ordinal: 2,
					status: "completed",
					type: "activity",
				}),
				item({
					...base,
					id: "diagnostic_2",
					ordinal: 3,
					summary: "Usage update",
					type: "native_event",
				}),
			],
			true,
		);

		expect(segments.filter((segment) => segment.type === "diagnostic_group")).toEqual([
			expect.objectContaining({
				items: [
					expect.objectContaining({ id: "diagnostic_1" }),
					expect.objectContaining({ id: "diagnostic_2" }),
				],
				type: "diagnostic_group",
			}),
		]);
	});

	it("always surfaces diagnostics for failed work, overriding the preference", () => {
		const failure_diagnostic = item({
			...base,
			id: "diagnostic_failure",
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
			'let open = $state(item.status === "failed" || item.status === "cancelled");',
		);
		expect(work_session).toContain('is_failed ? "text-destructive" : ""');
		expect(trace).toContain(
			"make_conversation_trace_segments(items, $conversation_diagnostics_enabled, failed)",
		);
		expect(trace).toContain(">Failure details</span>");
		expect(trace).toContain('role={failed ? "alert" : undefined}');
		expect(workspace).toContain('failed={block.session.status === "failed" ||');
	});
});

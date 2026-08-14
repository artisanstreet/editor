import { describe, expect, it } from "vitest";

import {
	normalize_claude_event,
	read_claude_stream_message_id,
	type ClaudeNormalizationInput,
} from "../../modules/engines/src/claude/normalizer";
import {
	claude_assistant_text_frame,
	claude_content_block_start_frame,
	claude_hook_response_frame,
	claude_hook_started_frame,
	claude_init_frame,
	claude_message_start_frame,
	claude_rate_limit_allowed_frame,
	claude_rate_limit_exceeded_frame,
	claude_signature_delta_frame,
	claude_sonnet5_empty_thinking_assistant_frame,
	claude_status_frame,
	claude_terminal_failure_frame,
	claude_terminal_result_frame,
	claude_text_delta_frame,
	claude_thinking_and_text_assistant_frame,
	claude_thinking_delta_frame,
	claude_thinking_only_assistant_frame,
	claude_thinking_tokens_frame,
} from "./fixtures/claude-stream-frames";

const normalize = (payload: unknown, overrides: Partial<ClaudeNormalizationInput> = {}) =>
	normalize_claude_event({
		artisan_run_id: "run_1",
		frame_sequence: 1,
		native_thread_id: "claude-session",
		payload,
		raw_frame_base64: "",
		turn_id: "claude:run_1:turn",
		...overrides,
	});

describe("claude stream-json normalization", () => {
	it("keeps bookkeeping frames out of the canonical stream", () => {
		for (const frame of [
			claude_hook_started_frame,
			claude_hook_response_frame,
			claude_status_frame,
			claude_thinking_tokens_frame,
			claude_message_start_frame,
			claude_content_block_start_frame,
			claude_signature_delta_frame,
			claude_rate_limit_allowed_frame,
		]) {
			expect(normalize(frame), JSON.stringify(frame).slice(0, 60)).toEqual([]);
		}
	});

	it("emits a summary-free compaction marker at the native stream boundary", () => {
		const [observation] = normalize({
			compactMetadata: { trigger: "auto" },
			type: "system",
			subtype: "compact_boundary",
			uuid: "boundary-1",
		});

		expect(observation).toMatchObject({
			_tag: "compaction",
			compaction_id: "boundary-1",
			raw: {
				native_id: "boundary-1",
				native_method: "system.compact_boundary",
			},
			state: "completed",
		});
		expect(JSON.stringify(observation)).not.toContain("summary");
	});

	it("reports the run as running once the CLI initializes", () => {
		expect(normalize(claude_init_frame)).toEqual([
			expect.objectContaining({ _tag: "run_state", state: "running" }),
		]);
	});

	it("streams thinking as a canonical reasoning summary", () => {
		const [observation] = normalize(claude_thinking_delta_frame);

		expect(observation).toMatchObject({
			_tag: "reasoning_summary_delta",
			delta: "The user has sent",
			summary_index: 0,
		});
	});

	it("correlates streamed text and its completion onto one item", () => {
		const message_id = read_claude_stream_message_id(claude_message_start_frame);
		expect(message_id).toBe("msg_011CdVSx52pWZx8S1tJU7uoQ");
		if (message_id === undefined) throw new Error("message_start must announce an id");

		const [delta] = normalize(claude_text_delta_frame, {
			stream_message_id: message_id,
		});
		const [completed] = normalize(claude_assistant_text_frame, {
			stream_message_id: message_id,
		});

		expect(delta).toMatchObject({
			_tag: "agent_message_delta",
			delta: "I'm not sure what you'd like",
		});
		expect(completed).toMatchObject({ _tag: "agent_message_completed" });
		/** Divergent ids would upsert a second assistant message beside the streamed one. */
		expect((delta as { item_id: string }).item_id).toBe(
			(completed as { item_id: string }).item_id,
		);
	});

	it("falls back to a run-stable item when no message was announced", () => {
		const [delta] = normalize(claude_text_delta_frame);

		expect((delta as { item_id: string }).item_id).toBe("claude:run_1:message");
	});

	it("closes the reasoning phase when a thinking-only assistant frame settles", () => {
		expect(normalize(claude_thinking_only_assistant_frame)).toEqual([
			expect.objectContaining({
				_tag: "reasoning_summary_completed",
				item_id: "msg_011CdVSx52pWZx8S1tJU7uoQ:reasoning",
				turn_id: "claude:run_1:turn",
			}),
			expect.objectContaining({ _tag: "usage" }),
		]);
	});

	it("closes a suppressed-display reasoning phase even when no delta ever streamed", () => {
		const message_id = read_claude_stream_message_id(claude_message_start_frame);
		if (message_id === undefined) throw new Error("message_start must announce an id");

		const [delta] = normalize(claude_thinking_delta_frame, { stream_message_id: message_id });
		const observations = normalize(claude_sonnet5_empty_thinking_assistant_frame, {
			stream_message_id: message_id,
		});

		expect(observations).toEqual([
			expect.objectContaining({
				_tag: "reasoning_summary_completed",
				item_id: (delta as { item_id: string }).item_id,
				turn_id: "claude:run_1:turn",
			}),
			expect.objectContaining({ _tag: "usage" }),
		]);
	});

	it("emits the reasoning completion before the message completion when a frame carries both", () => {
		expect(normalize(claude_thinking_and_text_assistant_frame)).toEqual([
			expect.objectContaining({
				_tag: "reasoning_summary_completed",
				item_id: "msg_011CdVSx52pWZx8S1tJU7uoQ:reasoning",
			}),
			expect.objectContaining({
				_tag: "agent_message_completed",
				item_id: "msg_011CdVSx52pWZx8S1tJU7uoQ",
				message: "I'm not sure what you'd like me to help with.",
			}),
			expect.objectContaining({ _tag: "usage" }),
		]);
	});

	it("adds no reasoning completion when an assistant frame carries no thinking blocks", () => {
		expect(normalize(claude_assistant_text_frame)).toEqual([
			expect.objectContaining({ _tag: "agent_message_completed" }),
			expect.objectContaining({ _tag: "usage", context_tokens: expect.any(Number) }),
		]);
	});

	it("normalizes the type-less terminal frame into usage", () => {
		expect(normalize(claude_terminal_result_frame)).toEqual([
			expect.objectContaining({
				_tag: "usage",
				basis: "cumulative",
				input_tokens: 10,
				output_tokens: 290,
			}),
		]);
	});

	it("surfaces a terminal failure and an exhausted rate limit", () => {
		const failure = normalize(claude_terminal_failure_frame);
		const rate_limited = normalize(claude_rate_limit_exceeded_frame);

		expect(failure).toContainEqual(
			expect.objectContaining({
				_tag: "native_action",
				detail: "Claude run failed: max_tokens",
			}),
		);
		expect(rate_limited).toEqual([
			expect.objectContaining({
				_tag: "native_action",
				detail: "Claude rate limit exceeded",
				error_ref: expect.objectContaining({
					artisan_code: "AE-PROVIDER-201",
					limit_id: "five_hour",
					limit_scope: "unknown",
				}),
			}),
		]);
	});

	it("still reports genuinely unknown frames instead of dropping them", () => {
		expect(normalize({ type: "some_future_event" })).toEqual([
			expect.objectContaining({
				_tag: "native_action",
				detail: "Unknown Claude event type: some_future_event",
			}),
		]);
	});
});

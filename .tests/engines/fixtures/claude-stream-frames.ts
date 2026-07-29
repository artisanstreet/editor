/**
 * Verbatim Claude Code stream-json frames captured from a real turn
 * (claude-haiku-4-5, CLI stream-json v1). Long opaque values are shortened,
 * but every envelope shape is exactly as the CLI emitted it.
 */

const session_id = "51e7b289-cd3a-4ef7-bfea-174d3d1fd696";

export const claude_hook_started_frame = {
	type: "system",
	subtype: "hook_started",
	hook_id: "9e22b16a-f9de-4bda-8c22-e3ac551a6614",
	hook_name: "SessionStart:startup",
	hook_event: "SessionStart",
	uuid: "81f78730-3a4b-4f1a-9866-1add6a647a41",
	session_id,
};

export const claude_hook_response_frame = {
	type: "system",
	subtype: "hook_response",
	hook_id: "9e22b16a-f9de-4bda-8c22-e3ac551a6614",
	hook_name: "SessionStart:startup",
	hook_event: "SessionStart",
	output: "MANDATORY: the following engineering standards apply",
	session_id,
};

export const claude_init_frame = {
	type: "system",
	subtype: "init",
	cwd: "C:\\Users\\sander\\Desktop\\artisan-editor",
	session_id,
	tools: ["Task", "Bash", "Edit"],
	model: "claude-haiku-4-5-20251001",
	permissionMode: "default",
	uuid: "3f0d9f4a-2c3d-4b0e-9f70-5f6f2f9a1b22",
};

export const claude_status_frame = {
	type: "system",
	subtype: "status",
	status: "requesting",
	uuid: "97dc5b97-0508-46e8-a818-4d72fd8582b8",
	session_id,
};

export const claude_thinking_tokens_frame = {
	type: "system",
	subtype: "thinking_tokens",
	estimated_tokens: 28,
	estimated_tokens_delta: 23,
	uuid: "bfd57e55-0100-449b-b133-230c17122d06",
	session_id,
};

export const claude_message_start_frame = {
	type: "stream_event",
	event: {
		type: "message_start",
		message: {
			model: "claude-haiku-4-5-20251001",
			id: "msg_011CdVSx52pWZx8S1tJU7uoQ",
			type: "message",
			role: "assistant",
			content: [],
			stop_reason: null,
			stop_sequence: null,
		},
	},
	session_id,
	parent_tool_use_id: null,
	uuid: "0f2f0b26-1f0e-4de6-9f2f-2fd0c9d5e4a1",
};

export const claude_content_block_start_frame = {
	type: "stream_event",
	event: {
		type: "content_block_start",
		index: 1,
		content_block: { type: "text", text: "" },
	},
	session_id,
	parent_tool_use_id: null,
	uuid: "6fdd1bff-1f12-4ac0-b27a-8bd00be9794e",
};

export const claude_thinking_delta_frame = {
	type: "stream_event",
	event: {
		type: "content_block_delta",
		index: 0,
		delta: { type: "thinking_delta", thinking: "The user has sent", estimated_tokens: null },
	},
	session_id,
	parent_tool_use_id: null,
	uuid: "5c6f9a10-7b2e-4d63-9d1e-1c53a0d4e2b7",
};

export const claude_signature_delta_frame = {
	type: "stream_event",
	event: {
		type: "content_block_delta",
		index: 0,
		delta: { type: "signature_delta", signature: "EsMICpMBCBAYAipAKM2hoTVt99Ff" },
	},
	session_id,
	parent_tool_use_id: null,
	uuid: "b6d1a0f5-1a8e-4a4a-9f77-0c2f1d3e4a55",
};

export const claude_text_delta_frame = {
	type: "stream_event",
	event: {
		type: "content_block_delta",
		index: 1,
		delta: { type: "text_delta", text: "I'm not sure what you'd like" },
	},
	session_id,
	parent_tool_use_id: null,
	uuid: "9a1c2d3e-4f50-4a61-8b72-0d9e8f7a6b5c",
};

export const claude_thinking_only_assistant_frame = {
	type: "assistant",
	message: {
		model: "claude-haiku-4-5-20251001",
		id: "msg_011CdVSx52pWZx8S1tJU7uoQ",
		type: "message",
		role: "assistant",
		content: [{ type: "thinking", thinking: "The user has sent a message", signature: "EsMI" }],
		stop_reason: null,
		usage: { input_tokens: 10, output_tokens: 4 },
	},
	parent_tool_use_id: null,
	session_id,
	uuid: "7c8d9e0f-1a2b-4c3d-8e4f-5a6b7c8d9e0f",
};

export const claude_assistant_text_frame = {
	type: "assistant",
	message: {
		model: "claude-haiku-4-5-20251001",
		id: "msg_011CdVSx52pWZx8S1tJU7uoQ",
		type: "message",
		role: "assistant",
		content: [
			{
				type: "text",
				text: "I'm not sure what you'd like me to help with.",
			},
		],
		stop_reason: null,
		stop_sequence: null,
		usage: { input_tokens: 10, output_tokens: 4 },
	},
	parent_tool_use_id: null,
	session_id,
	uuid: "2b3c4d5e-6f70-4819-a2b3-c4d5e6f70819",
};

export const claude_rate_limit_allowed_frame = {
	type: "rate_limit_event",
	rate_limit_info: {
		status: "allowed",
		resetsAt: 1_785_304_200,
		rateLimitType: "five_hour",
		overageStatus: "rejected",
		isUsingOverage: false,
	},
	uuid: "a07bb51f-c052-47e5-a209-b00be7b98a7f",
	session_id,
};

export const claude_rate_limit_exceeded_frame = {
	...claude_rate_limit_allowed_frame,
	rate_limit_info: { ...claude_rate_limit_allowed_frame.rate_limit_info, status: "exceeded" },
};

/** The current CLI emits its terminal summary without an envelope `type`. */
export const claude_terminal_result_frame = {
	is_error: false,
	duration_api_ms: 3_916,
	num_turns: 1,
	stop_reason: "end_turn",
	session_id,
	total_cost_usd: 0.020_974,
	usage: {
		input_tokens: 10,
		cache_creation_input_tokens: 8_689,
		cache_read_input_tokens: 21_360,
		output_tokens: 290,
		service_tier: "standard",
	},
};

export const claude_terminal_failure_frame = {
	...claude_terminal_result_frame,
	is_error: true,
	stop_reason: "max_tokens",
};

import type { EngineDescriptor } from "../engine";
import { claude_native_continuation_version } from "./probe";

export const claude_transport = "claude-cli-stream-json";
export const claude_protocol_version = "claude-stream-json-v1";

/** Declares the Claude capability surface reached through the installed CLI. @since 0.8.0 */
export const ClaudeEngineDescriptor: EngineDescriptor = {
	capabilities: {
		approval: {
			state: "supported",
			reason: "CLI stdio permission requests bridge to canonical approvals.",
		},
		auth: {
			state: "supported",
			reason: "Uses the installed Claude Code subscription session.",
		},
		cancel: { state: "supported" },
		close: { state: "supported" },
		events: { state: "supported" },
		global_guidance: {
			state: "unsupported",
			reason: "Claude Code reads its own global CLAUDE.md natively; no mirror is wired.",
		},
		model_selection: {
			state: "supported",
			reason: "Native model identifiers pass through to the CLI's --model option.",
		},
		native_continuation: {
			state: "supported",
			reason: `Same-engine model changes are explicitly target-model and Claude ${claude_native_continuation_version} gated.`,
		},
		native_tools: {
			state: "experimental",
			reason: "Known public tool activity is normalized; provider-native details remain raw.",
		},
		probe: { state: "supported", reason: "Only --version and auth status are probed." },
		question: {
			state: "supported",
			reason: "AskUserQuestion permission requests are canonicalized and answered in place.",
		},
		raw_frames: { state: "supported" },
		resume: { state: "supported" },
		start: { state: "supported" },
		steer: {
			state: "experimental",
			reason: "CLI stream-input messages fold into the active turn; Claude Code owns the timing.",
		},
		subagents: {
			state: "supported",
			reason: "CLI task lifecycle and parent_tool_use_id frames are canonicalized.",
		},
	},
	display_name: "Claude",
	id: "claude",
	transport: claude_transport,
};

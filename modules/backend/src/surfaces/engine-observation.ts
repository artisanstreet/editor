import type { EngineObservation } from "@artisan/engines";
import type { SurfaceItem } from "@artisan/protocol";

/** Maps normalized observations to public-safe canonical surface items. */
export const SurfaceFromEngineObservation = (
	observation: EngineObservation,
	attribution: SurfaceItem["attribution"],
	occurred_at: string,
): SurfaceItem => {
	const base = {
		attribution,
		occurred_at,
		raw_observation: {
			engine_id: observation.raw.engine_id,
			observation_id: observation.observation_id,
		},
		raw_origin: {
			provider: observation.raw.engine_id,
			reference: String(observation.raw.native_id ?? observation.observation_id),
		},
		surface_id: `surface:${observation.observation_id}`,
	};
	const summary = (label: string, status?: string) => ({
		label,
		...(status === undefined ? {} : { status }),
	});
	switch (observation._tag) {
		case "agent_message_delta":
		case "agent_message_completed":
		case "reasoning_summary_completed":
		case "reasoning_summary_delta":
			return {
				...base,
				category: "work",
				kind: "message",
				summary: summary("Agent message"),
			};
		case "plan":
			return { ...base, category: "work", kind: "plan", summary: summary("Plan") };
		case "run_state":
		case "run_terminal":
			return {
				...base,
				category: "work",
				kind: "run",
				summary: summary("Run", observation.state),
			};
		case "turn_state":
			return {
				...base,
				category: "work",
				kind: "turn",
				summary: summary("Turn", observation.state),
			};
		case "approval":
			return {
				...base,
				category: "permission",
				kind: "approval",
				summary: summary("Approval", observation.state),
			};
		case "question":
			return {
				...base,
				category: "work",
				kind: "message",
				summary: summary("Question", observation.state),
			};
		case "terminal_activity":
			return {
				...base,
				category: "process",
				kind: "terminal",
				summary: summary("Terminal", observation.state),
			};
		case "tool":
			return {
				...base,
				category: "capability",
				kind: "tool",
				summary: summary("Tool", observation.action),
			};
		case "file":
			return {
				...base,
				category: "change",
				kind: "file_observation",
				summary: summary("File", observation.action),
			};
		case "search":
			return {
				...base,
				category: "capability",
				kind: "web_search",
				summary: summary("Search", observation.state),
			};
		case "compaction":
			return {
				...base,
				category: "native_action",
				kind: "compaction",
				summary: summary("Compaction", observation.state),
			};
		case "retry":
			return {
				...base,
				category: "native_action",
				kind: "provider_diagnostic",
				summary: summary("Retry", observation.attempt_state),
			};
		case "protocol_diagnostic":
		case "process_diagnostic":
			return {
				...base,
				category: "native_action",
				kind: "provider_diagnostic",
				summary: summary("Provider diagnostic", observation.level),
			};
		case "native_action":
			return {
				...base,
				category: "native_action",
				kind: "engine_native_action",
				summary: summary("Engine action"),
			};
		case "usage":
			return {
				...base,
				category: "work",
				kind: "run",
				summary: summary("Usage", observation.basis),
			};
	}
};

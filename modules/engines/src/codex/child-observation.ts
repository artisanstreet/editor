import {
	type EngineObservation,
	type EngineSubagentObservation,
	MakeEngineSubagentTranscriptObservation,
} from "../engine";

function subagent_state_from_turn(
	state: "started" | "waiting" | "completed" | "cancelled" | "failed",
): EngineSubagentObservation["state"] {
	return state === "started"
		? "running"
		: state === "completed"
			? "completed"
			: state === "failed"
				? "failed"
				: state === "waiting"
					? "waiting"
					: "interrupted";
}

/** Maps a foreign Codex thread's activity without letting it own the root run. */
export function NormalizeCodexChildObservation(input: {
	readonly child_native_thread_id: string;
	readonly observation: EngineObservation;
	readonly parent_native_thread_id: string;
}): EngineObservation | undefined {
	const { observation } = input;
	if (observation._tag === "subagent") return observation;
	if (observation._tag === "turn_state")
		return {
			_tag: "subagent",
			agent_native_thread_id: input.child_native_thread_id,
			artisan_run_id: observation.artisan_run_id,
			observation_id: `${observation.observation_id}:subagent`,
			parent_native_thread_id: input.parent_native_thread_id,
			raw: observation.raw,
			sequence: observation.raw.frame_sequence ?? observation.sequence,
			state: subagent_state_from_turn(observation.state),
			turn_id: observation.turn_id,
		} satisfies EngineSubagentObservation;
	if (observation._tag === "run_state")
		return {
			_tag: "subagent",
			agent_native_thread_id: input.child_native_thread_id,
			artisan_run_id: observation.artisan_run_id,
			observation_id: `${observation.observation_id}:subagent`,
			parent_native_thread_id: input.parent_native_thread_id,
			raw: observation.raw,
			sequence: observation.raw.frame_sequence ?? observation.sequence,
			state: observation.state === "running" ? "running" : "waiting",
		} satisfies EngineSubagentObservation;
	if (
		observation._tag === "run_terminal" ||
		(observation._tag === "native_action" && observation.action === "thread/closed")
	)
		return {
			_tag: "subagent",
			agent_native_thread_id: input.child_native_thread_id,
			artisan_run_id: observation.artisan_run_id,
			observation_id: `${observation.observation_id}:subagent`,
			parent_native_thread_id: input.parent_native_thread_id,
			raw: observation.raw,
			sequence: observation.raw.frame_sequence ?? observation.sequence,
			state:
				observation._tag === "run_terminal" && observation.state === "completed"
					? "completed"
					: observation._tag === "run_terminal" && observation.state === "failed"
						? "failed"
						: "interrupted",
		} satisfies EngineSubagentObservation;
	return MakeEngineSubagentTranscriptObservation({
		agent_native_thread_id: input.child_native_thread_id,
		observation,
		parent_native_thread_id: input.parent_native_thread_id,
	});
}

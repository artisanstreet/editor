/** Tracks whether resumed-thread usage belongs to a newly opened Artisan turn. */
export interface CodexResumedUsageState {
	readonly expected_turn_id: string | undefined;
	readonly new_turn_started: boolean;
	readonly resumed: boolean;
}

export function make_codex_resumed_usage_state(resumed: boolean): CodexResumedUsageState {
	return {
		expected_turn_id: undefined,
		new_turn_started: !resumed,
		resumed,
	};
}

export function accept_codex_turn_start(
	state: CodexResumedUsageState,
	turn_id: string,
): CodexResumedUsageState {
	return state.resumed ? { ...state, expected_turn_id: turn_id } : state;
}

export function observe_codex_turn_started(
	state: CodexResumedUsageState,
	turn_id: string,
): CodexResumedUsageState {
	return state.resumed && state.expected_turn_id === turn_id
		? { ...state, new_turn_started: true }
		: state;
}

export function is_codex_resume_usage_baseline(state: CodexResumedUsageState): boolean {
	return state.resumed && !state.new_turn_started;
}

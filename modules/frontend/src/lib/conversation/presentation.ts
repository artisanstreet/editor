/** Neither details nor failure state opens settled history; only currently live work starts open. */
export const work_session_initially_open = (input: {
	readonly has_details: boolean;
	readonly unsuccessful: boolean;
	readonly working: boolean;
}): boolean => input.working;

/**
 * The visual contract for a work-session disclosure. Keeping it pure makes the
 * active-state exception explicit: active work keeps its detail tree mounted,
 * but liveness never gets to choose whether the reader has it open.
 */
export const work_session_disclosure = (input: {
	readonly details_defined: boolean;
	readonly has_visible_details: boolean;
	readonly open: boolean;
	readonly working: boolean;
}) => {
	const controllable = input.has_visible_details;
	const visible = controllable || input.working;

	return {
		can_collapse: controllable,
		data_open: visible ? input.open : undefined,
		data_state: visible ? (input.open ? "open" : "closed") : undefined,
		details_hidden: !input.open,
		details_mounted: input.details_defined && (input.working || input.open),
	};
};

export type ModelTransitionPresentation = "pending_source" | "target_only" | "source_and_target";

/**
 * A handoff can be announced before the prior provider has reported its model.
 * Do not briefly claim that it changed only *to* the target: the completion
 * observation can fill that source in on the same durable transition item.
 */
export const model_transition_presentation = (
	state: "started" | "completed",
	source_model_id: string | undefined,
): ModelTransitionPresentation => {
	if (state === "started" && source_model_id === undefined) return "pending_source";
	return source_model_id === undefined ? "target_only" : "source_and_target";
};

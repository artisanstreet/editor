/** A submitted steer remains visible until its own authoritative settlement. */
export interface PendingSteeringLip<Submission> {
	readonly generation: number;
	readonly started_at: number;
	readonly submission: Submission;
}

/** Local ownership state for the stack of pending steering acknowledgement lips. */
export interface SteeringPendingLipState<Submission> {
	readonly next_generation: number;
	/** Newest first: the stack renders top-down from the latest steer. */
	readonly pending: ReadonlyArray<PendingSteeringLip<Submission>>;
}

/** Stacks a new lip on top without giving any older settlement authority over it. */
export const begin_pending_steering_lip = <Submission>(
	state: SteeringPendingLipState<Submission>,
	submission: Submission,
	started_at: number,
): {
	readonly begun: PendingSteeringLip<Submission>;
	readonly state: SteeringPendingLipState<Submission>;
} => {
	const begun = {
		generation: state.next_generation + 1,
		started_at,
		submission,
	};
	return {
		begun,
		state: {
			next_generation: begun.generation,
			pending: [begun, ...state.pending],
		},
	};
};

/** Releases only the lip owned by the settling receipt; the rest of the stack stands. */
export const release_pending_steering_lip = <Submission>(
	state: SteeringPendingLipState<Submission>,
	generation: number,
): { readonly released: boolean; readonly state: SteeringPendingLipState<Submission> } => {
	const pending = state.pending.filter((lip) => lip.generation !== generation);
	if (pending.length === state.pending.length) return { released: false, state };
	return {
		released: true,
		state: { ...state, pending },
	};
};

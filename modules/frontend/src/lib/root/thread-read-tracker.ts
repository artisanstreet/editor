import type { ThreadListItem } from "@artisan/protocol";

import { ThreadHasActiveWork, ThreadReaderActivityAt } from "./thread-navigation";

export interface ThreadReadTrackingState {
	readonly observed_activity_at?: string;
	readonly route_id?: string;
	readonly thread?: ThreadListItem;
}

export interface ThreadReadAcknowledgement {
	readonly reader_activity_at: string;
	readonly thread_id: string;
}

/** Advances visible-thread tracking and acknowledges only a real route departure. */
export const AdvanceThreadReadTracking = (
	previous: ThreadReadTrackingState,
	input: {
		readonly root_visible: boolean;
		readonly route_id: string | undefined;
		readonly thread: ThreadListItem | undefined;
	},
): {
	readonly acknowledgement?: ThreadReadAcknowledgement;
	readonly state: ThreadReadTrackingState;
} => {
	const route_changed = previous.route_id !== input.route_id;
	if (!route_changed) {
		const thread = input.thread ?? previous.thread;
		return {
			state: {
				...(previous.route_id === undefined ? {} : { route_id: previous.route_id }),
				...(thread === undefined ? {} : { thread }),
				...(input.root_visible && thread !== undefined
					? { observed_activity_at: ThreadReaderActivityAt(thread) }
					: previous.observed_activity_at === undefined
						? {}
						: { observed_activity_at: previous.observed_activity_at }),
			},
		};
	}

	const state: ThreadReadTrackingState = {
		...(input.route_id === undefined ? {} : { route_id: input.route_id }),
		...(input.thread === undefined ? {} : { thread: input.thread }),
		...(input.root_visible && input.thread !== undefined
			? { observed_activity_at: ThreadReaderActivityAt(input.thread) }
			: {}),
	};
	const observed_activity_at = previous.observed_activity_at;
	const departed = previous.thread;
	if (
		departed === undefined ||
		observed_activity_at === undefined ||
		ThreadHasActiveWork(departed) ||
		departed.reader_acknowledged_activity_at === observed_activity_at
	)
		return { state };

	return {
		acknowledgement: {
			reader_activity_at: observed_activity_at,
			thread_id: departed.thread_id,
		},
		state,
	};
};

const sidebar_enter_stagger_ms = 40;
const sidebar_enter_stagger_window_ms = 250;
const sidebar_exit_stagger_ms = 28;
const sidebar_exit_stagger_window_ms = 150;

export type SidebarFlipMeasurement = {
	left: number;
	top: number;
};

const get_scaled_stagger_step_ms = (
	child_count: number,
	stagger_ms: number,
	stagger_window_ms: number,
) => Math.min(stagger_ms, stagger_window_ms / (child_count - 1));

/** Keeps sidebar reveal and exit staggers inside fixed motion windows as the list grows. */
export const get_sidebar_stagger_delays = (child_index: number, child_count: number) => {
	if (child_count <= 1) {
		return { enter_ms: 0, exit_ms: 0 };
	}

	const enter_step_ms = get_scaled_stagger_step_ms(
		child_count,
		sidebar_enter_stagger_ms,
		sidebar_enter_stagger_window_ms,
	);
	const exit_step_ms = get_scaled_stagger_step_ms(
		child_count,
		sidebar_exit_stagger_ms,
		sidebar_exit_stagger_window_ms,
	);

	return {
		enter_ms: Math.round(child_index * enter_step_ms),
		exit_ms: Math.round((child_count - child_index - 1) * exit_step_ms),
	};
};

/** Returns the compositor translation that preserves an element's previous position. */
export const get_sidebar_flip_translation = (
	before: SidebarFlipMeasurement,
	after: SidebarFlipMeasurement,
) => ({
	x: before.left - after.left,
	y: before.top - after.top,
});

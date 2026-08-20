/**
 * A damped spring, integrated a frame at a time.
 *
 * Duration-based easing is the wrong model for anything the pointer is
 * steering. A curve has to finish before it can be asked to go somewhere else,
 * so a second input either queues behind the first or restarts it from zero. A
 * spring carries its velocity through a change of target, which is why a fast
 * run of clicks retargets mid-flight instead of stuttering, and why a flick can
 * mean more than where the fingers happened to stop.
 *
 * Pure arithmetic, and only that. The measuring and playing that goes with it —
 * reading a rectangle either side of a layout change and animating the
 * difference — deliberately does not live here. That work is only correct
 * inside a pointer handler between `tick()` and the next paint, so it stays in
 * the component that owns the frame.
 */

export type SpringState = { readonly value: number; readonly velocity: number };

export const SpringStep = (
	state: SpringState,
	target: number,
	dt: number,
	stiffness: number,
	damping: number,
): SpringState => {
	const acceleration = (target - state.value) * stiffness - state.velocity * damping;
	const velocity = state.velocity + acceleration * dt;
	return { value: state.value + velocity * dt, velocity };
};

/** Near enough, and slow enough, that another frame would not be visible. */
export const SpringSettled = (state: SpringState, target: number, epsilon = 0.0005): boolean =>
	Math.abs(target - state.value) < epsilon && Math.abs(state.velocity) < epsilon;

import { MediaQuery } from "svelte/reactivity";

const default_mobile_breakpoint = 768;

/**
 * Reactive media query helper for mobile-width detection.
 */
export class IsMobile extends MediaQuery {
	constructor(breakpoint: number = default_mobile_breakpoint) {
		super(`(max-width: ${breakpoint - 1}px)`);
	}
}

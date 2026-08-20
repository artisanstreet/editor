/**
 * The one piece of motion machinery the second-pass drafts share.
 *
 * It is no longer draft-local: the spring graduated into the application when
 * Orbit became the root landing surface, so the drafts read the same arithmetic
 * the shipped arc runs on rather than a copy that could drift away from it.
 */

export { SpringSettled, SpringStep, type SpringState } from "$lib/motion/spring";

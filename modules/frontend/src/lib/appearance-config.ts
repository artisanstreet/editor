import { writable } from "svelte/store";

/**
 * Whether glass surfaces light themselves with the shader.
 *
 * A store rather than a service because every glass surface is an ordinary
 * component that reads it while rendering; the durable value behind it lives in
 * `AppearancePreferences`, which the shell loads once at startup and the
 * settings screen writes. Starts enabled so the first frame matches the design
 * even before the stored preference has been read.
 */
export const shader_enabled = writable(true);

/**
 * How wide the reading column runs. The names describe the feel, not pixels;
 * the widths they resolve to live with the other design tokens in global.css.
 */
export type ProseWidth = "tight" | "balanced" | "loose";

/**
 * The chosen prose width, defaulting to the width the transcript was designed
 * at. Same shape as `shader_enabled`: the durable value lives in
 * `AppearancePreferences`; this store is what the shell paints from.
 */
export const prose_width = writable<ProseWidth>("balanced");

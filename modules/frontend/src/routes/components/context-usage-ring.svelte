<script lang="ts">
	import { ContextGaugeToneMix } from "$lib/context-usage/gauge-tone";

	/**
	 * Presentational context-window gauge. Purely a mark: it carries no control
	 * and no tooltip of its own because it is drawn inside the model picker's
	 * trigger, where a nested button would be invalid and would steal the hover
	 * from the pill it sits in. The detail behind it lives in that trigger's
	 * tooltip; see `context-usage-details.svelte`.
	 *
	 * The percent is derived by the caller because the window denominator may
	 * come from the provider wire or from the catalog fallback, and
	 * `compaction_percent` with it because where an engine compacts is a
	 * property of the selected harness and model.
	 */
	let {
		compaction_percent,
		percent,
	}: {
		readonly compaction_percent: number;
		readonly percent: number;
	} = $props();

	const ring_radius = 6;
	const ring_circumference = 2 * Math.PI * ring_radius;
	const tone = $derived(ContextGaugeToneMix(percent, compaction_percent));
</script>

<!--
	The arc carries its own tone rather than the trigger's `currentColor`: a mark
	that means something has to keep meaning it while the pill lights on hover,
	and muted-on-muted was unreadable at this size. The track is struck from the
	foreground so it stays visible on either theme, where `--muted` vanished into
	the composer surface.
-->
<svg
	viewBox="0 0 16 16"
	class="context-gauge size-4 shrink-0 -rotate-90"
	style={`--gauge-warn: ${tone.warn}%; --gauge-danger: ${tone.danger}%`}
	aria-hidden="true"
>
	<circle class="[stroke:color-mix(in_oklch,var(--foreground)_16%,transparent)]" cx="8" cy="8" r={ring_radius} fill="none" stroke-width="2" />
	<circle
		class="[transition-property:stroke-dasharray,stroke] duration-(--duration-quick) ease-(--ease-smooth-out) [stroke:color-mix(in_oklch,var(--banner-error)_var(--gauge-danger,0%),color-mix(in_oklch,var(--banner-warning)_var(--gauge-warn,0%),var(--banner-info)))]"
		cx="8"
		cy="8"
		r={ring_radius}
		fill="none"
		stroke-width="2.5"
		stroke-linecap="round"
		stroke-dasharray={`${(percent / 100) * ring_circumference} ${ring_circumference}`}
	/>
</svg>


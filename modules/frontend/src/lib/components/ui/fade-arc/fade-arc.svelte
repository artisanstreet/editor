<script lang="ts">
	import type { SVGAttributes } from "svelte/elements";
	import { cn } from "$lib/utils";

	/**
	 * Ported from loading-ui's FadeArc (loading-ui.com): a spinning arc whose
	 * leading half fades toward full current-color and whose trailing half
	 * dissolves to nothing, so the spin reads as a comet rather than a ring.
	 * It sizes from the caller's classes and inherits `currentColor`; set
	 * `--duration` on the element to retime the spin.
	 */
	let {
		class: class_name = undefined,
		...rest
	}: SVGAttributes<SVGSVGElement> = $props();

	/** Gradient ids must be unique per instance or parallel arcs share paint. */
	const uid = $props.id();
</script>

<svg
	viewBox="0 0 24 24"
	fill="none"
	xmlns="http://www.w3.org/2000/svg"
	role="status"
	class={cn("animate-[fade-arc-spin_var(--duration,1s)_linear_infinite]", class_name)}
	{...rest}
>
	<defs>
		<linearGradient id="{uid}-leading" x1="50%" x2="50%" y1="5.271%" y2="91.793%">
			<stop offset="0%" stop-color="currentColor" />
			<stop offset="100%" stop-color="currentColor" stop-opacity="0.55" />
		</linearGradient>
		<linearGradient id="{uid}-trailing" x1="50%" x2="50%" y1="15.24%" y2="87.15%">
			<stop offset="0%" stop-color="currentColor" stop-opacity="0" />
			<stop offset="100%" stop-color="currentColor" stop-opacity="0.55" />
		</linearGradient>
	</defs>
	<g fill="none">
		<path
			d="M8.749.021a1.5 1.5 0 0 1 .497 2.958A7.5 7.5 0 0 0 3 10.375a7.5 7.5 0 0 0 7.5 7.5v3c-5.799 0-10.5-4.7-10.5-10.5C0 5.23 3.726.865 8.749.021"
			fill="url(#{uid}-leading)"
			transform="translate(1.5 1.625)"
		/>
		<path
			d="M15.392 2.673a1.5 1.5 0 0 1 2.119-.115A10.48 10.48 0 0 1 21 10.375c0 5.8-4.701 10.5-10.5 10.5v-3a7.5 7.5 0 0 0 5.007-13.084a1.5 1.5 0 0 1-.115-2.118"
			fill="url(#{uid}-trailing)"
			transform="translate(1.5 1.625)"
		/>
	</g>
</svg>


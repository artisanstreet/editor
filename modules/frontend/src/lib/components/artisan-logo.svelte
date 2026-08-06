<script lang="ts">
	interface ArtisanLogoProps {
		/** Row font size in pixels; the whole mark scales from it. */
		readonly size?: number;
	}

	const { size = 48 }: ArtisanLogoProps = $props();

	/**
	 * The stacked company wordmark tuned in /debug/logo: one shared size and
	 * tracking, per-row line heights that interlock the three rows. Color
	 * inherits from the surrounding text.
	 */
	const rows = [
		{ line_height: 0.71, text: "AR" },
		{ line_height: 0.8, text: "TIS" },
		{ line_height: 0.7, text: "AN" },
	] as const;
</script>

<div
	class="flex flex-col items-center"
	role="img"
	aria-label="Artisan"
	style:font-family="'Sigurd Variable', serif"
	style:font-size="{size}px"
	style:font-weight={400}
	style:letter-spacing="-0.05em"
>
	{#each rows as row (row.text)}
		<div class="text-center" style:line-height={row.line_height}>{row.text}</div>
	{/each}
</div>

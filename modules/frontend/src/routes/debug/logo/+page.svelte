<script lang="ts">
	import { dev } from "$app/environment";
	import ArtisanLogo from "$lib/components/artisan-logo.svelte";

	const canvas_color = "#0a0a0a";
	/** Row line heights sum to 2.21, so this size renders the mark h-40 (160px) tall. */
	const size = 160 / 2.21;
</script>

<svelte:head><title>Logo lab</title></svelte:head>

{#if !dev}
	<div class="flex h-full items-center justify-center p-10">
		<p class="text-sm text-muted-foreground">
			This surface belongs to development tooling and is not part of this build.
		</p>
	</div>
{:else}
	<!-- Above the Forge connection gate (z-50): this page needs no Forge at all. -->
	<div
		class="fixed inset-0 z-[60] flex flex-wrap items-center justify-center gap-24 overflow-auto p-8"
		style:background-color={canvas_color}
	>
		<div class="h-40 text-neutral-50">
			<ArtisanLogo {size} />
		</div>

		<div class="flex w-fit flex-col overflow-hidden rounded-xl border-2 border-muted-foreground">
			<!--
				The mark's box is the cutout: the canvas shows through the panel
				while the letters keep the tile's own color. The window carries its
				own padding because the interlocked rows overshoot their line boxes
				— without it the ascenders paint tile-on-tile and vanish.
			-->
			<div class="p-4 text-muted-foreground" style:background-color={canvas_color}>
				<ArtisanLogo {size} />
			</div>
			<!-- Half the mark's h-40 below the window. -->
			<div class="h-20 w-full bg-muted-foreground"></div>
		</div>
	</div>
{/if}

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
		class="fixed inset-0 z-[60] flex items-center justify-center overflow-auto p-8"
		style:background-color={canvas_color}
	>
		<div class="h-40 text-surface-50">
			<ArtisanLogo {size} />
		</div>
	</div>
{/if}

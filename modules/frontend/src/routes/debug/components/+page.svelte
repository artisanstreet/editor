<script lang="ts">
	import { dev } from "$app/environment";
	import { page } from "$app/state";
	import ChevronLeft from "@tabler/icons-svelte/icons/chevron-left";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import { Button } from "$lib/components/ui/button";
	import {
		component_gallery_entries,
		component_gallery_index_for,
		component_gallery_neighbor,
		type ComponentGalleryId,
	} from "./catalog";
	import ComponentPreview from "./component-preview.svelte";

	const requested_id = $derived(page.url.searchParams.get("component"));
	const index = $derived(component_gallery_index_for(requested_id));
	const entry = $derived(component_gallery_entries[index]);
	const previous = $derived(component_gallery_neighbor(index, -1));
	const next = $derived(component_gallery_neighbor(index, 1));

	const component_href = (id: ComponentGalleryId): string =>
		`/debug/components?component=${encodeURIComponent(id)}`;
</script>

<!--
	Unguarded because it cannot be otherwise: `svelte:head` may not sit inside a
	block. The production build stubs this whole page, so the name never ships.
-->
<svelte:head><title>Thread component gallery</title></svelte:head>

{#if !dev}
	<div class="flex h-full items-center justify-center p-10">
		<p class="text-sm text-muted-foreground">
			This surface belongs to development tooling and is not part of this build.
		</p>
	</div>
{:else if entry !== undefined}
	<!-- Above the Forge connection gate: every specimen is backed by local mock data. -->
	<main class="fixed inset-0 z-[60] overflow-y-auto bg-background text-foreground">
		<header
			class="fixed top-4 left-1/2 z-[70] flex w-[min(42rem,calc(100vw-2rem))] -translate-x-1/2 flex-col items-center"
		>
			<nav
				class="card-lg flex max-w-full items-center gap-1 rounded-full border border-border bg-background/90 p-1 shadow-lg shadow-black/5 backdrop-blur-xl"
				aria-label="Thread component gallery"
			>
				<Button
					href={component_href(previous.id)}
					variant="ghost"
					size="icon-sm"
					aria-label={`Previous component: ${previous.label}`}
				>
					<ChevronLeft />
				</Button>

				<div class="flex min-w-0 flex-1 flex-col items-center px-3 py-0.5 text-center">
					<span class="text-[10px] leading-none text-muted-foreground">{entry.group}</span>
					<span class="mt-1 flex min-w-0 items-baseline gap-2 leading-none">
						<h1 class="truncate text-sm font-medium text-foreground">{entry.label}</h1>
						<span class="shrink-0 font-mono text-[11px] tabular-nums text-muted-foreground">
							{index + 1}/{component_gallery_entries.length}
						</span>
					</span>
				</div>

				<Button
					href={component_href(next.id)}
					variant="ghost"
					size="icon-sm"
					aria-label={`Next component: ${next.label}`}
				>
					<ChevronRight />
				</Button>
			</nav>

			<p class="mt-2 max-w-xl px-4 text-center text-xs leading-5 text-muted-foreground">
				{entry.description}
			</p>
		</header>

		<section
			class="mx-auto flex min-h-full w-full items-center justify-center px-6 pt-32 pb-10 sm:px-10"
			aria-labelledby="component-preview-heading"
		>
			<h2 id="component-preview-heading" class="sr-only">{entry.label} preview</h2>
			{#key entry.id}
				<ComponentPreview id={entry.id} />
			{/key}
		</section>
	</main>
{/if}

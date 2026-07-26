<script lang="ts">
	import type { Snippet } from "svelte";
	import LayoutSidebar from "@tabler/icons-svelte/icons/layout-sidebar";

	import * as Sidebar from "$lib/components/ui/sidebar";

	let {
		primary,
		secondary,
		sidebar,
	}: {
		primary: Snippet;
		secondary?: Snippet;
		sidebar: Snippet;
	} = $props();
</script>

<Sidebar.Provider
	class="h-full"
	mobile_breakpoint={1024}
	style="--sidebar-width: 16rem; --sidebar-width-icon: 2.5rem; min-height: 0;"
>
	<Sidebar.Root variant="inset" collapsible="icon">
		<div class="relative flex min-h-0 flex-1 flex-row">
			{@render sidebar()}
			<Sidebar.Trigger
				class="group/sidebar-toggle absolute right-0 top-2 hidden size-10 items-center justify-center rounded-full bg-surface-125 card lg:flex dark:bg-surface-900"
			>
				<LayoutSidebar
					class="size-4 text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover/sidebar-toggle:text-foreground motion-reduce:transition-none"
				/>
			</Sidebar.Trigger>
		</div>
	</Sidebar.Root>

	<Sidebar.Inset
		class="min-h-full min-w-0 w-0 flex-1 p-2 lg:h-[calc(100%-1rem)] lg:min-h-0 lg:max-h-[calc(100%-1rem)] lg:peer-data-[collapsible=icon]:pl-0"
		style="padding-bottom: max(0.5rem, env(safe-area-inset-bottom));"
	>
		<div
			class="docs-responsive-surfaces flex h-full min-h-0 flex-row items-stretch justify-between gap-2 overflow-visible"
		>
			<section
				class="min-h-0 min-w-0 flex-1 rounded-3xl bg-linear-to-b from-surface-125 to-surface-75 p-1 card dark:from-surface-900 dark:to-surface-925"
			>
				{@render primary()}
			</section>

			{#if secondary}
				<section
					class="min-h-0 w-[clamp(16rem,25vw,350px)] shrink-0 rounded-3xl bg-linear-to-b from-surface-125 to-surface-75 p-1 card dark:from-surface-900 dark:to-surface-925"
				>
					{@render secondary()}
				</section>
			{/if}
		</div>
	</Sidebar.Inset>
</Sidebar.Provider>

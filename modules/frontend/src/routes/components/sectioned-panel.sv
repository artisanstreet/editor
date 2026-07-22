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
	mobile_breakpoint={1024}
	style="--sidebar-width: 16rem; --sidebar-width-icon: 2.5rem;"
>
	<Sidebar.Root variant="inset" collapsible="icon">
		<div class="relative flex min-h-0 flex-1 flex-row">
			{@render sidebar()}
			<Sidebar.Trigger
				class="group/sidebar-toggle absolute right-0 top-2 hidden size-10 items-center justify-center rounded-full bg-foreground/5 card lg:flex"
			>
				<LayoutSidebar
					class="size-4 text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover/sidebar-toggle:text-foreground motion-reduce:transition-none"
				/>
			</Sidebar.Trigger>
		</div>
	</Sidebar.Root>

	<Sidebar.Inset
		class="min-h-dvh min-w-0 w-0 flex-1 p-2 lg:h-[calc(100dvh-1rem)] lg:min-h-0 lg:max-h-[calc(100dvh-1rem)] lg:peer-data-[collapsible=icon]:pl-0"
		style="padding-bottom: max(0.5rem, env(safe-area-inset-bottom));"
	>
		<div
			class="docs-responsive-surfaces flex h-full min-h-0 flex-row items-stretch justify-between gap-2 overflow-visible"
		>
			<section
				class="min-h-0 min-w-0 flex-1 rounded-3xl bg-linear-to-b from-foreground/5 to-foreground/2.5 p-1 card"
			>
				{@render primary()}
			</section>

			{#if secondary}
				<section
					class="min-h-0 w-[clamp(16rem,25vw,350px)] shrink-0 rounded-3xl bg-linear-to-b from-foreground/5 to-foreground/2.5 p-1 card"
				>
					{@render secondary()}
				</section>
			{/if}
		</div>
	</Sidebar.Inset>
</Sidebar.Provider>

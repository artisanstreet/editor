<script lang="ts">
	import * as Sheet from "$lib/components/ui/sheet/index.js";
	import { cn, type WithElementRef } from "$lib/utils.js";
	import { play } from "cuelume";
	import type { HTMLAttributes } from "svelte/elements";
	import { sidebar_width_mobile } from "$lib/components/ui/sidebar/constants.js";
	import { use_sidebar } from "$lib/components/ui/sidebar/context.svelte.js";

	let {
		ref = $bindable(null),
		side = "left",
		variant = "sidebar",
		collapsible = "offcanvas",
		class: class_name,
		children,
		...rest_props
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		side?: "left" | "right";
		variant?: "sidebar" | "floating" | "inset";
		collapsible?: "offcanvas" | "icon" | "none";
	} = $props();

	const sidebar = use_sidebar();
</script>

{#if collapsible === "none"}
	<div
		class={cn(
			"bg-background text-foreground flex h-full w-(--sidebar-width) flex-col",
			class_name
		)}
		bind:this={ref}
		{...rest_props}
	>
		{@render children?.()}
	</div>
{:else if sidebar.is_mobile}
	<Sheet.Root
		bind:open={() => sidebar.open_mobile, (value) => sidebar.set_open_mobile(value)}
		{...rest_props}
	>
		<Sheet.Content
			bind:ref
			motion="none"
			data-sidebar="sidebar"
			data-slot="sidebar"
			data-mobile="true"
			class={cn(
				"t-panel-slide-x bg-background text-foreground inset-y-2 left-0 h-auto w-[min(var(--sidebar-width),calc(100vw-0.5rem))] rounded-l-none rounded-r-2xl border-0 p-0 card",
				class_name
			)}
			closeButtonClass="top-2 right-2 size-10 rounded-full bg-foreground/5 text-muted-foreground card hover:text-foreground"
			onCloseButtonClick={() => play("toggle")}
			style="--sidebar-width: {sidebar_width_mobile};"
			{side}
		>
			<Sheet.Header class="sr-only">
				<Sheet.Title>Sidebar</Sheet.Title>
				<Sheet.Description>Displays the mobile sidebar.</Sheet.Description>
			</Sheet.Header>
			<div class="flex h-full w-full flex-col">
				{@render children?.()}
			</div>
		</Sheet.Content>
	</Sheet.Root>
{:else}
	<div
		bind:this={ref}
		class="text-foreground group peer hidden md:block"
		data-state={sidebar.state}
		data-collapsible={sidebar.state === "collapsed" ? collapsible : ""}
		data-motion-phase={sidebar.motion_phase}
		data-variant={variant}
		data-side={side}
		data-slot="sidebar"
	>
		<div
			data-slot="sidebar-gap"
			class={cn(
				"relative w-(--sidebar-width) bg-transparent",
				"group-data-[collapsible=offcanvas]:w-0",
				"group-data-[side=right]:rotate-180",
				variant === "floating" || variant === "inset"
					? "group-data-[collapsible=icon]:w-[calc(var(--sidebar-width-icon)+(--spacing(4)))]"
					: "group-data-[collapsible=icon]:w-(--sidebar-width-icon)"
			)}
		></div>
		<div
			data-slot="sidebar-container"
			class={cn(
				"t-sidebar-motion fixed inset-y-0 z-10 hidden h-svh w-(--sidebar-width) md:flex",
				side === "left" ? "start-0" : "end-0",
				variant === "floating" || variant === "inset"
					? "p-2"
					: "group-data-[side=left]:border-e group-data-[side=right]:border-s",
				class_name
			)}
			{...rest_props}
		>
			<div
				data-sidebar="sidebar"
				data-slot="sidebar-inner"
				class="bg-background group-data-[variant=floating]:ring-sidebar-border group-data-[variant=floating]:rounded-lg group-data-[variant=floating]:shadow-sm group-data-[variant=floating]:ring-1 flex size-full flex-col"
			>
				{@render children?.()}
			</div>
		</div>
	</div>
{/if}

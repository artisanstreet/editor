<script lang="ts">
	import { cn, type WithElementRef } from "$lib/utils.js";
	import type { Snippet } from "svelte";
	import type { HTMLAnchorAttributes } from "svelte/elements";

	let {
		ref = $bindable(null),
		children,
		child,
		class: class_name,
		size = "md",
		isActive: is_active = false,
		...rest_props
	}: WithElementRef<HTMLAnchorAttributes> & {
		child?: Snippet<[{ props: Record<string, unknown> }]>;
		size?: "sm" | "md";
		isActive?: boolean;
	} = $props();

	const merged_props = $derived({
		class: cn(
			"text-foreground ring-sidebar-ring hover:bg-sidebar-accent hover:text-sidebar-accent-foreground active:bg-sidebar-accent active:text-sidebar-accent-foreground [&>svg]:text-sidebar-accent-foreground data-active:bg-sidebar-accent data-active:text-sidebar-accent-foreground h-7 gap-2 rounded-md px-2 focus-visible:ring-2 data-[size=md]:text-sm data-[size=sm]:text-xs [&>svg]:size-4 flex min-w-0 -translate-x-px items-center overflow-hidden outline-hidden group-data-[collapsible=icon]:hidden disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50 [&>span:last-child]:truncate [&>svg]:shrink-0",
			class_name
		),
		"data-slot": "sidebar-menu-sub-button",
		"data-sidebar": "menu-sub-button",
		"data-size": size,
		"data-active": is_active,
		...rest_props,
	});
</script>

{#if child}
	{@render child({ props: merged_props })}
{:else}
	<a bind:this={ref} {...merged_props}>
		{@render children?.()}
	</a>
{/if}

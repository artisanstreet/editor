<script lang="ts">
	import { cn, type WithElementRef } from "$lib/utils.js";
	import type { Snippet } from "svelte";
	import type { HTMLAttributes } from "svelte/elements";

	let {
		ref = $bindable(null),
		children,
		child,
		class: class_name,
		...rest_props
	}: WithElementRef<HTMLAttributes<HTMLElement>> & {
		child?: Snippet<[{ props: Record<string, unknown> }]>;
	} = $props();

	const merged_props = $derived({
		class: cn(
			"text-foreground/70 ring-sidebar-ring h-8 rounded-md px-3 text-xs font-medium transition-opacity duration-200 ease-linear group-data-[collapsible=icon]:-mt-8 group-data-[collapsible=icon]:opacity-0 focus-visible:ring-2 [&>svg]:size-4 flex shrink-0 items-center outline-hidden [&>svg]:shrink-0",
			class_name
		),
		"data-slot": "sidebar-group-label",
		"data-sidebar": "group-label",
		...rest_props,
	});
</script>

{#if child}
	{@render child({ props: merged_props })}
{:else}
	<div bind:this={ref} {...merged_props}>
		{@render children?.()}
	</div>
{/if}

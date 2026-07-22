<script lang="ts">
	import { cn, type WithElementRef } from "$lib/utils.js";
	import type { Snippet } from "svelte";
	import type { HTMLButtonAttributes } from "svelte/elements";

	let {
		ref = $bindable(null),
		class: class_name,
		children,
		child,
		...rest_props
	}: WithElementRef<HTMLButtonAttributes> & {
		child?: Snippet<[{ props: Record<string, unknown> }]>;
	} = $props();

	const merged_props = $derived({
		class: cn(
			"text-foreground ring-sidebar-ring hover:bg-sidebar-accent hover:text-sidebar-accent-foreground absolute top-3.5 right-3 w-5 rounded-md p-0 focus-visible:ring-2 [&>svg]:size-4 flex aspect-square items-center justify-center outline-hidden transition-transform group-data-[collapsible=icon]:hidden after:absolute after:-inset-2 md:after:hidden [&>svg]:shrink-0",
			class_name
		),
		"data-slot": "sidebar-group-action",
		"data-sidebar": "group-action",
		...rest_props,
	});
</script>

{#if child}
	{@render child({ props: merged_props })}
{:else}
	<button bind:this={ref} {...merged_props}>
		{@render children?.()}
	</button>
{/if}

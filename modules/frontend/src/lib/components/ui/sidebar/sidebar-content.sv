<script lang="ts">
	import type { HTMLAttributes } from "svelte/elements";
	import { cn, type WithElementRef } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: class_name,
		children,
		...rest_props
	}: WithElementRef<HTMLAttributes<HTMLElement>> = $props();
</script>

<div
	bind:this={ref}
	data-slot="sidebar-content"
	data-sidebar="content"
	class={cn(
		"no-scrollbar gap-2 [--radius:var(--radius-xl)] flex min-h-0 flex-1 flex-col overflow-auto group-data-[collapsible=icon]:overflow-hidden",
		class_name
	)}
	{...rest_props}
>
	{@render children?.()}
</div>

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
	data-slot="sidebar-group"
	data-sidebar="group"
	class={cn("p-2 relative flex w-full min-w-0 flex-col", class_name)}
	{...rest_props}
>
	{@render children?.()}
</div>

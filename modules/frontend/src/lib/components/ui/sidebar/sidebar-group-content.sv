<script lang="ts">
	import { cn, type WithElementRef } from "$lib/utils.js";
	import type { HTMLAttributes } from "svelte/elements";

	let {
		ref = $bindable(null),
		class: class_name,
		children,
		...rest_props
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> = $props();
</script>

<div
	bind:this={ref}
	data-slot="sidebar-group-content"
	data-sidebar="group-content"
	class={cn("text-sm w-full", class_name)}
	{...rest_props}
>
	{@render children?.()}
</div>

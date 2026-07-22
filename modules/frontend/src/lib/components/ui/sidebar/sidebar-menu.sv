<script lang="ts">
	import { cn, type WithElementRef } from "$lib/utils.js";
	import type { HTMLAttributes } from "svelte/elements";

	let {
		ref = $bindable(null),
		class: class_name,
		children,
		...rest_props
	}: WithElementRef<HTMLAttributes<HTMLUListElement>, HTMLUListElement> = $props();
</script>

<ul
	bind:this={ref}
	data-slot="sidebar-menu"
	data-sidebar="menu"
	class={cn("gap-1 flex w-full min-w-0 flex-col", class_name)}
	{...rest_props}
>
	{@render children?.()}
</ul>

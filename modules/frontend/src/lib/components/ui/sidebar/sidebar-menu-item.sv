<script lang="ts">
	import { cn, type WithElementRef } from "$lib/utils.js";
	import type { HTMLAttributes } from "svelte/elements";

	let {
		ref = $bindable(null),
		class: class_name,
		children,
		...rest_props
	}: WithElementRef<HTMLAttributes<HTMLLIElement>, HTMLLIElement> = $props();
</script>

<li
	bind:this={ref}
	data-slot="sidebar-menu-item"
	data-sidebar="menu-item"
	class={cn("group/menu-item relative", class_name)}
	{...rest_props}
>
	{@render children?.()}
</li>

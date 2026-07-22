<script lang="ts">
	import { Separator } from "$lib/components/ui/separator/index.js";
	import { cn } from "$lib/utils.js";
	import type { ComponentProps } from "svelte";

	let {
		ref = $bindable(null),
		class: class_name,
		...rest_props
	}: ComponentProps<typeof Separator> = $props();
</script>

<Separator
	bind:ref
	data-slot="sidebar-separator"
	data-sidebar="separator"
	class={cn("bg-sidebar-border mx-2 w-auto", class_name)}
	{...rest_props}
/>

<script lang="ts">
	import type { HTMLOptionAttributes } from "svelte/elements";
	import { cn, type WithElementRef } from "$lib/utils";

	let {
		ref = $bindable(null),
		class: class_name,
		children,
		...rest_props
	}: WithElementRef<HTMLOptionAttributes> = $props();
</script>

<option
	bind:this={ref}
	data-slot="native-select-option"
	class={cn("bg-[Canvas] text-[CanvasText]", class_name)}
	{...rest_props}
>
	{@render children?.()}
</option>

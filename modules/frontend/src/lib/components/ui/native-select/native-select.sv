<script lang="ts">
	import Selector from "@tabler/icons-svelte/icons/selector";
	import type { HTMLSelectAttributes } from "svelte/elements";
	import { cn, type WithElementRef } from "$lib/utils";

	type NativeSelectProps = Omit<WithElementRef<HTMLSelectAttributes>, "size"> & {
		size?: "sm" | "default";
	};

	let {
		ref = $bindable(null),
		value = $bindable(),
		class: class_name,
		size = "default",
		children,
		...rest_props
	}: NativeSelectProps = $props();
</script>

<div
	class={cn(
		"group/native-select relative w-fit has-[select:disabled]:opacity-50",
		class_name,
	)}
	data-slot="native-select-wrapper"
	data-size={size}
>
	<select
		bind:value
		bind:this={ref}
		data-slot="native-select"
		data-size={size}
		class="h-9 w-full min-w-0 appearance-none rounded-4xl border border-input bg-input/30 py-1 pr-8 pl-3 text-sm transition-colors select-none outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:pointer-events-none disabled:cursor-not-allowed data-[size=sm]:h-8 dark:bg-input/30 dark:hover:bg-input/50"
		{...rest_props}
	>
		{@render children?.()}
	</select>
	<Selector
		class="pointer-events-none absolute top-1/2 right-3.5 size-4 -translate-y-1/2 text-muted-foreground select-none"
		aria-hidden="true"
		data-slot="native-select-icon"
	/>
</div>

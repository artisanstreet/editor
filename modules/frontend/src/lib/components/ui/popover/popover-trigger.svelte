<script lang="ts">
	import { Popover as PopoverPrimitive } from "bits-ui";

	import { cn } from "$lib/utils";

	let {
		ref = $bindable(null),
		class: class_name,
		...rest_props
	}: PopoverPrimitive.TriggerProps = $props();
</script>

<PopoverPrimitive.Trigger
	bind:ref
	data-slot="popover-trigger"
	class={cn("", class_name)}
	{...rest_props}
/>

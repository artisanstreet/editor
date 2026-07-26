<script lang="ts">
	import type { ComponentProps } from "svelte";
	import { Popover as PopoverPrimitive } from "bits-ui";

	import PopoverPortal from "$lib/components/ui/popover/popover-portal.sv";
	import { cn, type WithoutChildrenOrChild } from "$lib/utils";

	let {
		ref = $bindable(null),
		class: class_name,
		sideOffset: side_offset = 4,
		align = "center",
		portalProps: portal_props,
		variant = "default",
		...rest_props
	}: PopoverPrimitive.ContentProps & {
		portalProps?: WithoutChildrenOrChild<ComponentProps<typeof PopoverPortal>>;
		variant?: "bare" | "default";
	} = $props();
</script>

<PopoverPortal {...portal_props}>
	<PopoverPrimitive.Content
		bind:ref
		data-slot="popover-content"
		sideOffset={side_offset}
		{align}
		class={cn(
			"text-popover-foreground data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 flex flex-col text-sm duration-100 data-[side=inline-start]:slide-in-from-right-2 data-[side=inline-end]:slide-in-from-left-2 z-50 origin-(--transform-origin) outline-hidden",
			variant === "default" && "card bg-popover gap-4 rounded-2xl p-4 w-72",
			class_name,
		)}
		{...rest_props}
	/>
</PopoverPortal>

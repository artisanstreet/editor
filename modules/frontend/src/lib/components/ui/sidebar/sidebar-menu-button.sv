<script lang="ts" module>
	import { tv, type VariantProps } from "tailwind-variants";

	/**
	 * Tailwind variants for sidebar menu buttons.
	 */
	export const sidebar_menu_button_variants = tv({
		base: "ring-sidebar-ring hover:bg-sidebar-accent hover:text-sidebar-accent-foreground active:bg-sidebar-accent active:text-sidebar-accent-foreground data-active:bg-sidebar-accent data-active:text-sidebar-accent-foreground data-open:hover:bg-sidebar-accent data-open:hover:text-sidebar-accent-foreground gap-2 rounded-lg px-3 py-2 text-left text-sm transition-colors group-has-data-[sidebar=menu-action]/menu-item:pr-8 group-data-[collapsible=icon]:size-8! group-data-[collapsible=icon]:p-2! focus-visible:ring-2 data-active:font-medium peer/menu-button group/menu-button flex w-full items-center overflow-hidden outline-hidden disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50 [&_svg]:size-4 [&_svg]:shrink-0 [&>span:last-child]:truncate",
		variants: {
			variant: {
				default: "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
				outline: "bg-background hover:bg-sidebar-accent hover:text-sidebar-accent-foreground shadow-[0_0_0_1px_var(--sidebar-border)] hover:shadow-[0_0_0_1px_var(--sidebar-accent)]",
			},
			size: {
				default: "h-9 text-sm",
				sm: "h-8 text-xs",
				lg: "h-14 px-3 text-sm group-data-[collapsible=icon]:p-0!",
			},
		},
		defaultVariants: {
			variant: "default",
			size: "default",
		},
	});

	/**
	 * Variant names supported by sidebar menu buttons.
	 */
	export type SidebarMenuButtonVariant = VariantProps<
		typeof sidebar_menu_button_variants
	>["variant"];

	/**
	 * Size names supported by sidebar menu buttons.
	 */
	export type SidebarMenuButtonSize = VariantProps<typeof sidebar_menu_button_variants>["size"];
</script>

<script lang="ts">
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import { cn, type WithElementRef, type WithoutChildrenOrChild } from "$lib/utils.js";
	import { mergeProps } from "bits-ui";
	import type { ComponentProps, Snippet } from "svelte";
	import type { HTMLAttributes } from "svelte/elements";
	import { use_sidebar } from "$lib/components/ui/sidebar/context.svelte.js";

	let {
		ref = $bindable(null),
		class: class_name,
		children,
		child,
		variant = "default",
		size = "default",
		isActive: is_active = false,
		tooltipContent: tooltip_content,
		tooltipContentProps: tooltip_content_props,
		...rest_props
	}: WithElementRef<HTMLAttributes<HTMLButtonElement>, HTMLButtonElement> & {
		isActive?: boolean;
		variant?: SidebarMenuButtonVariant;
		size?: SidebarMenuButtonSize;
		tooltipContent?: Snippet | string;
		tooltipContentProps?: WithoutChildrenOrChild<ComponentProps<typeof Tooltip.Content>>;
		child?: Snippet<[{ props: Record<string, unknown> }]>;
	} = $props();

	const sidebar = use_sidebar();

	const button_props = $derived({
		class: cn(sidebar_menu_button_variants({ variant, size }), class_name),
		"data-slot": "sidebar-menu-button",
		"data-sidebar": "menu-button",
		"data-size": size,
		"data-active": is_active,
		...rest_props,
	});
</script>

{#snippet Button({ props }: { props?: Record<string, unknown> })}
	{@const merged_props = mergeProps(button_props, props)}
	{#if child}
		{@render child({ props: merged_props })}
	{:else}
		<button bind:this={ref} {...merged_props}>
			{@render children?.()}
		</button>
	{/if}
{/snippet}

{#if !tooltip_content}
	{@render Button({})}
{:else}
	<Tooltip.Root>
		<Tooltip.Trigger>
			{#snippet child({ props })}
				{@render Button({ props })}
			{/snippet}
		</Tooltip.Trigger>
		<Tooltip.Content
			side="right"
			align="center"
			hidden={sidebar.state !== "collapsed" || sidebar.is_mobile}
			{...tooltip_content_props}
		>
			{#if typeof tooltip_content === "string"}
				{tooltip_content}
			{:else if tooltip_content}
				{@render tooltip_content()}
			{/if}
		</Tooltip.Content>
	</Tooltip.Root>
{/if}

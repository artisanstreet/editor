<script lang="ts" module>
	import { cn, type WithElementRef } from "$lib/utils.js";
	import type { HTMLAnchorAttributes, HTMLButtonAttributes } from "svelte/elements";

	export type ButtonVariant =
		| "default"
		| "outline"
		| "secondary"
		| "ghost"
		| "destructive"
		| "link";
	export type ButtonSize = "default" | "xs" | "sm" | "lg" | "icon" | "icon-xs" | "icon-sm" | "icon-lg";

	export const buttonVariants = ({
		variant = "default",
		size = "default",
	}: {
		variant?: ButtonVariant;
		size?: ButtonSize;
	} = {}) =>
		cn(
			"focus-visible:border-ring focus-visible:ring-ring/50 rounded-4xl border border-transparent bg-clip-padding text-sm font-medium focus-visible:ring-[3px] active:not-aria-[haspopup]:translate-y-px inline-flex shrink-0 items-center justify-center whitespace-nowrap transition-all outline-none select-none disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
			{
				default: "bg-primary text-primary-foreground hover:bg-primary/80",
				outline: "border-border bg-input/30 hover:bg-input/50 hover:text-foreground aria-expanded:bg-muted aria-expanded:text-foreground",
				secondary: "bg-secondary text-secondary-foreground hover:bg-secondary/80",
				ghost: "hover:bg-muted hover:text-foreground",
				destructive: "bg-destructive/10 hover:bg-destructive/20 text-destructive",
				link: "text-primary underline-offset-4 hover:underline",
			}[variant],
			{
				default: "h-9 gap-1.5 px-3",
				xs: "h-6 gap-1 px-2.5 text-xs",
				sm: "h-8 gap-1 px-3",
				lg: "h-10 gap-1.5 px-4",
				icon: "size-9",
				"icon-xs": "size-6",
				"icon-sm": "size-8",
				"icon-lg": "size-10",
			}[size],
		);

	export type ButtonProps = WithElementRef<HTMLButtonAttributes> &
		WithElementRef<HTMLAnchorAttributes> & {
			variant?: ButtonVariant;
			size?: ButtonSize;
		};
</script>

<script lang="ts">
	let {
		class: className,
		variant = "default",
		size = "default",
		ref = $bindable(null),
		href = undefined,
		type = "button",
		disabled,
		children,
		...restProps
	}: ButtonProps = $props();
</script>

{#if href}
	<a
		bind:this={ref}
		data-slot="button"
		class={cn(buttonVariants({ variant, size }), className)}
		href={disabled ? undefined : href}
		aria-disabled={disabled}
		role={disabled ? "link" : undefined}
		tabindex={disabled ? -1 : undefined}
		{...restProps}
	>
		{@render children?.()}
	</a>
{:else}
	<button
		bind:this={ref}
		data-slot="button"
		class={cn(buttonVariants({ variant, size }), className)}
		{type}
		{disabled}
		{...restProps}
	>
		{@render children?.()}
	</button>
{/if}

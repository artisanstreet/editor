<script lang="ts">
	import type { Snippet } from "svelte";
	import { Tooltip, TooltipContent, TooltipTrigger } from "$lib/components/ui/tooltip";
	import ShaderGlassSurface from "../shader-glass-surface.svelte";

	let {
		advisory,
		children,
		description,
	}: {
		/**
		 * The cost or caveat the reader has to see before choosing, rendered
		 * ahead of the description in the destructive tone. Kept separate from
		 * the prose because a warning that opens a paragraph in the same colour
		 * as the paragraph is read at the same weight as the paragraph.
		 */
		advisory?: string;
		/** The row this describes — a `SelectItem`, given the trigger's props. */
		children: Snippet;
		description?: string;
	} = $props();
</script>

<Tooltip>
	<TooltipTrigger>
		{#snippet child({ props })}
			<span {...props} class="flex">
				{@render children()}
			</span>
		{/snippet}
	</TooltipTrigger>
	<!--
		Wears the dropdown's own surface rather than the default inverted pill, so
		a description reads as part of the menu it belongs to instead of as a
		foreign label floating beside it. The caret is dropped because it can only
		be painted in a solid fill: a glass surface has no way to continue into
		one, and faking the join reads as exactly that.
	-->
	<TooltipContent
		arrow={false}
		side="right"
		sideOffset={8}
		class="block max-w-80 rounded-2xl bg-transparent! p-0! text-foreground! shadow-none! ring-0!"
	>
		<ShaderGlassSurface strength="strong" class="w-full rounded-2xl" use_rays={false}>
			<span class="block text-pretty px-3 py-2 text-xs text-muted-foreground">
				{#if advisory !== undefined}
					<span class="font-medium text-destructive">{advisory}</span>{" "}
				{/if}{description}
			</span>
		</ShaderGlassSurface>
	</TooltipContent>
</Tooltip>

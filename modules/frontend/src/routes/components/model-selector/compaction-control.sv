<script lang="ts" effect>
	import ArrowsMinimize from "@tabler/icons-svelte/icons/arrows-minimize";
	import { MakeFollowHighlight } from "$lib/components/dropdown-highlight";
	import { Select, SelectContent, SelectItem, SelectTrigger } from "$lib/components/ui/select";
	import { Tooltip, TooltipContent, TooltipTrigger } from "$lib/components/ui/tooltip";
	import type { ModelChoice } from "$lib/engine/model-selection";
	import DropdownHoverSurface from "../dropdown-hover-surface.sv";
	import ShaderGlassSurface from "../shader-glass-surface.sv";

	const FollowHighlight = yield* MakeFollowHighlight;

	let {
		disabled,
		model,
		models,
		onselect,
		thread_model_value,
	}: {
		disabled: boolean;
		model?: ModelChoice;
		models: ReadonlyArray<ModelChoice>;
		onselect: (value: string) => void;
		thread_model_value: string;
	} = $props();
</script>

<div
	class="card flex items-center justify-between gap-2 rounded-lg bg-linear-to-b from-surface-225 to-surface-200 py-1 pr-1 pl-2.5 dark:from-surface-800 dark:to-surface-925"
>
	<Tooltip>
		<TooltipTrigger>
			{#snippet child({ props: tooltip_props })}
				<span {...tooltip_props} class="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
					<ArrowsMinimize class="size-3.5 shrink-0" aria-hidden="true" />
					<span class="truncate">Compaction model</span>
				</span>
			{/snippet}
		</TooltipTrigger>
		<TooltipContent side="top" class="max-w-64">
			Writes the summary that carries a thread's context across an engine or model switch.
			Thread model summarizes with whichever model the thread was already running.
		</TooltipContent>
	</Tooltip>
	<Select
		type="single"
		value={model?.id ?? thread_model_value}
		onValueChange={onselect}
		{disabled}
	>
		<SelectTrigger
			size="sm"
			class="h-6 w-40 shrink-0 border-transparent bg-transparent px-2 text-xs shadow-none data-[size=sm]:h-6 dark:bg-transparent dark:hover:bg-transparent dark:hover:text-foreground"
			aria-label="Compaction model"
		>
			<span class="truncate">{model?.name ?? "Thread model"}</span>
		</SelectTrigger>
		<SelectContent class="rounded-2xl border-transparent bg-transparent p-0 shadow-none">
			<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
				<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
					{#snippet children({ move_hover })}
						<SelectItem
							value={thread_model_value}
							class="focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
							{@attach FollowHighlight(move_hover)}
						>
							Thread model
						</SelectItem>
						{#each models as choice (choice.id)}
							<SelectItem
								value={choice.id}
								class="focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
								{@attach FollowHighlight(move_hover)}
							>
								{choice.name}
							</SelectItem>
						{/each}
					{/snippet}
				</DropdownHoverSurface>
			</ShaderGlassSurface>
		</SelectContent>
	</Select>
</div>

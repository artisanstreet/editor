<script lang="ts">
	import {
		FormatContextWindowTokens,
		type ModelChoice,
	} from "$lib/engine/model-selection";

	let { model }: { model: ModelChoice } = $props();

	const context_window = $derived(
		model.definition.capabilities.context_window_tokens === undefined
			? undefined
			: FormatContextWindowTokens(model.definition.capabilities.context_window_tokens),
	);
</script>

<div class="flex min-w-0 flex-col gap-1">
	<div class="flex min-w-0 items-baseline justify-between gap-2">
		<span class="truncate text-sm font-semibold text-foreground">{model.name}</span>
		{#if context_window !== undefined}
			<span class="shrink-0 text-[10px] text-muted-foreground/75">{context_window}</span>
		{/if}
	</div>
	{#if model.definition.description !== undefined}
		<span class="text-pretty text-xs text-muted-foreground">
			{model.definition.description}
		</span>
	{/if}
</div>

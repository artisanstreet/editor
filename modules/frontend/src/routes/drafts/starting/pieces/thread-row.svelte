<script lang="ts">
	/**
	 * One recent thread. The dot carries state and the glyph carries engine, so
	 * the title never has to spend characters on either.
	 */
	import { EnginePresentation, ThreadStateTone, type DraftThread } from "../mock";

	let {
		class: klass = "",
		show_dot = true,
		thread,
	}: { class?: string; show_dot?: boolean; thread: DraftThread } = $props();
</script>

<button
	class={`dz-row dz-press-row dz-focus group flex w-full min-w-0 items-center gap-2.5 rounded-lg px-2.5 py-2 text-left outline-none ${klass}`}
	type="button"
>
	{#if show_dot}
		<span
			class="state-dot size-1.5 shrink-0 rounded-full"
			style:--state-dot-tone={ThreadStateTone[thread.state]}
		></span>
	{/if}
	<span
		aria-hidden="true"
		class="shrink-0 text-[11px] leading-none"
		style:color={EnginePresentation[thread.engine].tone}
	>
		{EnginePresentation[thread.engine].glyph}
	</span>
	<span
		class="min-w-0 flex-1 truncate text-sm text-foreground transition-colors duration-(--duration-fast) group-hover:text-foreground-extra"
	>
		{thread.title}
	</span>
	<span class="shrink-0 text-xs tabular-nums text-muted-foreground">{thread.when}</span>
</button>

<script lang="ts" effect>
	import { BannerService } from "$lib/banner/service";
	import { Badge } from "$lib/components/ui/badge";
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";
	import { model_transition_presentation } from "$lib/conversation/presentation";
	import { model_manifest } from "@artisan/catalog";
	import type { ConversationItem } from "@artisan/protocol";
	import ArrowsMinimize from "@tabler/icons-svelte/icons/arrows-minimize";
	import type { Snippet } from "svelte";

	let {
		item,
		trailing,
		size = "sm",
	}: {
		item: Extract<
			ConversationItem,
			{ type: "error" | "compaction" | "model_transition" | "native_event" }
		>;
		trailing?: Snippet;
		/** "base" matches host text (the work-session header); timeline rows stay "sm". */
		size?: "sm" | "base";
	} = $props();

	const banner = yield* BannerService;
	if (item.type === "error") {
		yield* banner.error("Thread error", { description: item.message });
	}

	const timeline_status_class = $derived(
		`flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 py-0.5 ${item.type === "compaction" || size === "base" ? "text-base" : "text-sm"} text-muted-foreground`,
	);
	const engine_name_for = (engine_id: string) =>
		engine_id.charAt(0).toUpperCase() + engine_id.slice(1);
	/**
	 * Names a model, or nothing when the run never recorded which one it used —
	 * a run started on its engine's default keeps no model id. The engine name
	 * is not a stand-in here: "Changed Codex for GPT 5.6 Sol" reads as a swap of
	 * harnesses when only the model moved, and both sides may be the same engine.
	 */
	const model_name_for = (engine_id: string, model_id: string | undefined) =>
		model_id === undefined
			? undefined
			: (model_manifest.models.find(
					(candidate) =>
						candidate.harness === engine_id && candidate.native_model_id === model_id,
				)?.name ?? model_id);
</script>

{#if item.type === "model_transition"}
	{@const source_model_name = model_name_for(item.source_engine_id, item.source_model_id)}
	{@const target_model_name = model_name_for(item.target_engine_id, item.target_model_id)}
	{@const source_mark = EngineMarkFor(item.source_engine_id)}
	{@const target_mark = EngineMarkFor(item.target_engine_id)}
	{@const SourceIcon = source_mark.icon}
	{@const TargetIcon = target_mark.icon}
	{@const handing_over = item.state === "started"}
	{@const presentation = model_transition_presentation(item.state, item.source_model_id)}
	{#if presentation !== "pending_source"}
	<div
		class={timeline_status_class}
		data-conversation-status="model-transition"
		data-live-work-detail={handing_over ? "true" : undefined}
		role={handing_over ? "status" : undefined}
	>
		{#if handing_over}
			<!-- The next engine has not taken the thread yet, so nothing is thinking. -->
			<ShimmerText class="text-muted-foreground" delay={0} duration={2.4}>
				{source_model_name === undefined ? "Changing to" : "Changing"}
			</ShimmerText>
		{:else if presentation === "target_only"}
			<!-- Nothing to name on the way out, so the line states where the thread arrived. -->
			<span>Changed to</span>
		{:else}
			<span>Changed</span>
			<span class="inline-flex min-w-0 items-center gap-1.5 text-foreground">
				<SourceIcon class={EngineMarkClass(source_mark, "size-3.5")} aria-hidden="true" />
				<span class="sr-only">{engine_name_for(item.source_engine_id)} </span>
				<span class="truncate">{source_model_name}</span>
			</span>
			<span>for</span>
		{/if}
		<span class="inline-flex min-w-0 items-center gap-1.5 text-foreground">
			<TargetIcon class={EngineMarkClass(target_mark, "size-3.5")} aria-hidden="true" />
			<span class="sr-only">{engine_name_for(item.target_engine_id)} </span>
			<span class="truncate">{target_model_name ?? engine_name_for(item.target_engine_id)}</span>
		</span>
		{#if trailing !== undefined}{@render trailing()}{/if}
	</div>
	{/if}
{:else if item.type === "compaction"}
	<div
		class={timeline_status_class}
		data-conversation-status="compaction"
		data-live-work-detail={item.state === "started" ? "true" : undefined}
		role={item.state === "started" ? "status" : undefined}
		aria-label={item.state === "started"
			? "Compacting"
			: item.state === "failed"
				? "Compaction failed"
				: "Compacted"}
	>
		<ArrowsMinimize class="size-4 shrink-0" aria-hidden="true" />
		{#if item.state === "started"}
			<ShimmerText
				class="text-muted-foreground"
				delay={0}
				duration={2.4}
				aria-hidden="true"
			>
				Compacting
			</ShimmerText>
		{:else if item.state === "failed"}
			<span class="text-destructive">Compaction failed</span>
		{:else}
			<span>Compacted</span>
		{/if}
		{#if trailing !== undefined}{@render trailing()}{/if}
	</div>
{:else if item.type === "native_event"}
	<div class={timeline_status_class}>
		<Badge variant="outline">Native</Badge>
		<span>{item.summary}</span>
		{#if trailing !== undefined}{@render trailing()}{/if}
	</div>
{/if}

<script lang="ts" effect>
	import { BannerService } from "$lib/banner/service";
	import { Badge } from "$lib/components/ui/badge";
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";
	import { model_manifest } from "@artisan/catalog";
	import type { ConversationItem } from "@artisan/protocol";
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
		`flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 py-0.5 ${size === "base" ? "text-base" : "text-sm"} text-muted-foreground`,
	);
	const engine_name_for = (engine_id: string) =>
		engine_id.charAt(0).toUpperCase() + engine_id.slice(1);
	const model_name_for = (engine_id: string, model_id: string | undefined) =>
		model_id === undefined
			? engine_name_for(engine_id)
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
	<div class={timeline_status_class} data-conversation-status="model-transition">
		<span>Changed</span>
		<span class="inline-flex min-w-0 items-center gap-1.5 text-foreground">
			<SourceIcon class={EngineMarkClass(source_mark, "size-3.5")} aria-hidden="true" />
			<span class="sr-only">{engine_name_for(item.source_engine_id)} </span>
			<span class="truncate">{source_model_name}</span>
		</span>
		<span>for</span>
		<span class="inline-flex min-w-0 items-center gap-1.5 text-foreground">
			<TargetIcon class={EngineMarkClass(target_mark, "size-3.5")} aria-hidden="true" />
			<span class="sr-only">{engine_name_for(item.target_engine_id)} </span>
			<span class="truncate">{target_model_name}</span>
		</span>
		{#if trailing !== undefined}{@render trailing()}{/if}
	</div>
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

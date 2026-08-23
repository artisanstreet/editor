<script lang="ts" effect>
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import { Separator } from "$lib/components/ui/separator";
	import {
		EngineDisplayName,
		EngineMarkClass,
		PresentationForModelInCatalog,
	} from "$lib/engine/presentation";
	import { FormatElapsed } from "$lib/conversation/duration";
	import { OfflineRuntimeCatalog } from "$lib/runtime/offline-catalog";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import type { ConversationItem } from "@artisan/protocol";
	import type { RuntimeCatalog } from "@artisan/protocol";
	import { Effect, Option, Stream } from "effect";
	import type { Snippet } from "svelte";

	let {
		item,
		trailing,
		size = "sm",
		catalog,
	}: {
		item: Extract<
			ConversationItem,
			{ type: "error" | "compaction" | "model_transition" }
		>;
		trailing?: Snippet;
		/** "base" matches host text (the work-session header); timeline rows stay "sm". */
		size?: "sm" | "base";
		catalog?: RuntimeCatalog;
	} = $props();

	const defaults_controller = yield* Effect.serviceOption(SessionDefaultsController).pipe(
		Effect.map(Option.getOrUndefined),
	);
	let defaults_state = $state.raw<SessionDefaultsState | undefined>(
		yield* (defaults_controller === undefined
			? Effect.succeed(undefined)
			: defaults_controller.Current.pipe(Effect.orElseSucceed(() => undefined))),
	);
	/**
	 * Bound to a const rather than written inline: an inline handler makes this
	 * yield site read the very state it writes, which is the reactive loop that
	 * has taken the renderer down before.
	 */
	const ApplyDefaults = (next: SessionDefaultsState) =>
		Effect.sync(() => {
			defaults_state = next;
		});
	if (defaults_controller !== undefined) {
		yield* defaults_controller.Changes.pipe(
			Stream.runForEach(ApplyDefaults),
			Effect.forkScoped,
			Effect.orElseSucceed(() => undefined),
		);
	}

	const effective_catalog = $derived(catalog ?? defaults_state?.catalog ?? OfflineRuntimeCatalog);

	const timeline_status_class = $derived(
		`flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 py-0.5 ${size === "base" ? "text-base" : "text-sm"} text-muted-foreground`,
	);
</script>

{#if item.type === "model_transition"}
	{@const source_presentation = PresentationForModelInCatalog(
		item.source_engine_id,
		item.source_model_id,
		effective_catalog,
	)}
	{@const target_presentation = PresentationForModelInCatalog(
		item.target_engine_id,
		item.target_model_id,
		effective_catalog,
	)}
	{@const SourceIcon = source_presentation.mark.icon}
	{@const TargetIcon = target_presentation.mark.icon}
	{@const handing_over = item.state === "started"}
	<div
		class={timeline_status_class}
		data-conversation-status="model-transition"
		data-live-work-detail={handing_over ? "true" : undefined}
		role={handing_over ? "status" : undefined}
	>
		<!-- One persistent sentence prevents the pending and completed states from disagreeing. -->
		<ShimmerText
			active={handing_over}
			class="inline-flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 text-muted-foreground"
			delay={0}
			duration={2.4}
		>
			<span>{handing_over ? "Changing" : "Changed"}</span>
			<span class="inline-flex min-w-0 items-center gap-1.5 text-foreground">
				<SourceIcon class={EngineMarkClass(source_presentation.mark, "size-3.5")} aria-hidden="true" />
				<span class="sr-only">{EngineDisplayName(item.source_engine_id)} </span>
				<span class="truncate">{source_presentation.label}</span>
			</span>
			<span>for</span>
			<span class="inline-flex min-w-0 items-center gap-1.5 text-foreground">
				<TargetIcon class={EngineMarkClass(target_presentation.mark, "size-3.5")} aria-hidden="true" />
				<span class="sr-only">{EngineDisplayName(item.target_engine_id)} </span>
				<span class="truncate">{target_presentation.label}</span>
			</span>
		</ShimmerText>
		{#if trailing !== undefined}{@render trailing()}{/if}
	</div>
{:else if item.type === "compaction"}
	<div
		class="flex w-full min-w-0 flex-row items-center gap-4 py-0.5 text-base text-muted-foreground"
		data-conversation-status="compaction"
		data-live-work-detail={item.state === "started" ? "true" : undefined}
		role={item.state === "started" ? "status" : undefined}
		aria-label={item.state === "started"
			? "Compacting"
			: item.state === "failed"
				? "Compaction failed"
				: "Compacted"}
	>
		<Separator class="min-w-0 flex-1" aria-hidden="true" />
		<span class="flex shrink-0 items-center gap-1.5">
			<ShimmerText
				active={item.state === "started"}
				class={item.state === "failed"
					? "shrink-0 text-destructive"
					: "shrink-0 text-muted-foreground"}
				delay={0}
				duration={2.4}
				aria-hidden="true"
			>
				{item.state === "started"
					? "Compacting"
					: item.state === "failed"
						? "Compaction failed"
						: "Compacted"}
			</ShimmerText>
			<!--
				A compaction is the one event that can hold a transcript silent for
				minutes, and an engine that announces only the finished boundary
				leaves that silence unexplained. Naming the span is the whole
				answer to "why did nothing happen just now".
			-->
			{#if item.state !== "started" && item.duration_ms !== undefined}
				<span class="shrink-0 tabular-nums">· {FormatElapsed(item.duration_ms)}</span>
			{/if}
			{#if trailing !== undefined}{@render trailing()}{/if}
		</span>
		<Separator class="min-w-0 flex-1" aria-hidden="true" />
	</div>
{/if}

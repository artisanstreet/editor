<script lang="ts">
	import ConversationActivity from "$/components/conversation-activity.svelte";
	import ConversationApproval from "$/components/conversation-approval.svelte";
	import ConversationChangesCard from "$/components/conversation-changes-card.svelte";
	import ConversationErrorCard from "$/components/conversation-error-card.svelte";
	import ConversationMessage from "$/components/conversation-message.svelte";
	import ConversationPrompt from "$/components/conversation-prompt.svelte";
	import ConversationStatus from "$/components/conversation-status.svelte";
	import ConversationTrace from "$/components/conversation-trace.svelte";
	import ConversationTurnFooter from "$/components/conversation-turn-footer.svelte";
	import ConversationUsageInterruptionCard from "$/components/conversation-usage-interruption-card.svelte";
	import ConversationWorkSession from "$/components/conversation-work-session.svelte";
	import ContextUsageGauge from "$/components/context-usage-gauge.svelte";
	import ThreadWorkspace from "$/components/thread-workspace.svelte";
	import type { ComponentGalleryId } from "./catalog";
	import {
		gallery_active_activity,
		gallery_active_work,
		gallery_activity,
		gallery_assistant_message,
		gallery_approval,
		gallery_change_set,
		gallery_compacted,
		gallery_compacting,
		gallery_completed_work,
		gallery_context_usage,
		gallery_error_ref,
		gallery_failed_trace_items,
		gallery_failed_work,
		gallery_file_changes,
		gallery_image_message,
		gallery_model_handoff,
		gallery_question,
		gallery_reasoning_summary,
		gallery_streaming_message,
		gallery_thread_snapshot,
		gallery_trace_items,
		gallery_turn_settled_at,
		gallery_usage_continued,
		gallery_usage_limit,
		gallery_user_message,
	} from "./fixtures";

	let { id }: { readonly id: ComponentGalleryId } = $props();

	const image_sources = new Map([["gallery-artisan-mark", "/barekey-logo.png"]]);
</script>

<div class="flex min-h-full w-full items-center justify-center">
	{#if id === "full-thread"}
		<div
			class="h-[calc(100vh-10rem)] min-h-[32rem] w-full max-w-5xl overflow-hidden rounded-3xl border border-border bg-background shadow-2xl shadow-black/5"
		>
			<ThreadWorkspace snapshot={gallery_thread_snapshot} disabled />
		</div>
	{:else}
		<div class="w-full max-w-3xl">
			{#if id === "user-message"}
				<ConversationMessage item={gallery_user_message} />
			{:else if id === "image-message"}
				<ConversationMessage item={gallery_image_message} {image_sources} />
			{:else if id === "assistant-message"}
				<ConversationMessage item={gallery_assistant_message} />
			{:else if id === "streaming-message"}
				<ConversationMessage item={gallery_streaming_message} />
			{:else if id === "reasoning-summary"}
				<ConversationMessage item={gallery_reasoning_summary} />
			{:else if id === "active-work"}
				<ConversationWorkSession
					engine_id="codex"
					has_details
					item={gallery_active_work}
					run_authority="active"
				>
					{#snippet details()}
						<ConversationTrace items={gallery_trace_items} work_active />
					{/snippet}
				</ConversationWorkSession>
			{:else if id === "completed-work"}
				<ConversationWorkSession
					duration_kind="worked"
					item={gallery_completed_work}
				/>
			{:else if id === "failed-work"}
				<ConversationWorkSession
					has_details
					item={gallery_failed_work}
				>
					{#snippet details()}
						<ConversationTrace failed items={gallery_failed_trace_items} />
					{/snippet}
				</ConversationWorkSession>
			{:else if id === "activity-row"}
				<ConversationActivity item={gallery_activity} />
			{:else if id === "activity-trace"}
				<ConversationTrace
					items={[gallery_activity, gallery_active_activity, ...gallery_trace_items.slice(1)]}
					work_active
				/>
			{:else if id === "edited-files"}
				<ConversationChangesCard
					change_sets={[gallery_change_set]}
					files={gallery_file_changes}
					project_root_path="C:\\Users\\sander\\Desktop\\artisan-editor"
				/>
			{:else if id === "command-approval"}
				<ConversationApproval item={gallery_approval} />
			{:else if id === "question"}
				<ConversationPrompt item={gallery_question} />
			{:else if id === "usage-limit"}
				<ConversationUsageInterruptionCard interruption={gallery_usage_limit.interruption} />
			{:else if id === "usage-continued"}
				<ConversationUsageInterruptionCard interruption={gallery_usage_continued.interruption} />
			{:else if id === "provider-error"}
				<ConversationErrorCard error={gallery_error_ref} />
			{:else if id === "compacting"}
				<ConversationStatus item={gallery_compacting} size="base" />
			{:else if id === "compacted"}
				<ConversationStatus item={gallery_compacted} size="base" />
			{:else if id === "model-handoff"}
				<ConversationStatus item={gallery_model_handoff} size="base" />
			{:else if id === "context-window"}
				<div
					class="card mx-auto flex w-fit items-center gap-3 rounded-2xl bg-linear-to-b from-surface-100 to-surface-200 px-3 py-2 dark:from-surface-850 dark:to-surface-925"
				>
					<span class="text-sm text-muted-foreground">Claude Fable 5</span>
					<ContextUsageGauge
						compaction_percent={82}
						model_name="Claude Fable 5"
						percent={74.35}
						usage={gallery_context_usage}
						window_tokens={200_000}
					/>
				</div>
			{:else if id === "turn-actions"}
				<div class="group/turn relative pb-12">
					<ConversationMessage item={gallery_assistant_message} />
					<ConversationTurnFooter
						settled_at={gallery_turn_settled_at}
						text={gallery_assistant_message.text}
					/>
					<p class="absolute bottom-0 left-0 text-xs text-muted-foreground">
						Hover or focus the response to reveal its actions.
					</p>
				</div>
			{/if}
		</div>
	{/if}
</div>

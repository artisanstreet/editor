<script lang="ts">
	import type { ConversationItem, ImageAttachmentReference } from "@artisan/protocol";
	import type { Effect } from "effect";
	import ConversationActivity from "./conversation-activity.svelte";
	import ConversationApproval from "./conversation-approval.svelte";
	import ConversationChange from "./conversation-change.svelte";
	import ConversationMessage from "./conversation-message.svelte";
	import ConversationPrompt from "./conversation-prompt.svelte";
	import ConversationStatus from "./conversation-status.svelte";
	import ConversationUsageInterruptionCard from "./conversation-usage-interruption-card.svelte";
	import ConversationWorkSession from "./conversation-work-session.svelte";
	import type { Snippet } from "svelte";

	let {
		item,
		image_sources,
		message_streaming = false,
		onapproval,
		onimagevisibilitychange,
		onquestion,
		onusageinterruptionresolve,
		steering_pending = false,
		trailing,
	}: {
		item: ConversationItem;
		image_sources?: ReadonlyMap<string, string>;
		message_streaming?: boolean;
		onapproval?: (
			approval_id: string,
			approved: boolean,
		) => Effect.Effect<void, { readonly message: string }>;
		onimagevisibilitychange?: (
			attachments: ReadonlyArray<ImageAttachmentReference>,
			visible: boolean,
		) => Effect.Effect<void>;
		onquestion?: (
			question_id: string,
			answers: ReadonlyArray<string>,
		) => Effect.Effect<void, { readonly message: string }>;
		onusageinterruptionresolve?: (
			interruption_id: string,
			expected_revision: number,
			action:
				| { readonly type: "set_auto_continue"; readonly enabled: boolean }
				| {
						readonly type: "continue";
						readonly target_engine_id: string;
						readonly target_model_id?: string;
				  }
				| { readonly type: "cancel" },
		) => Effect.Effect<void, { readonly message: string }>;
		steering_pending?: boolean;
		trailing?: Snippet;
	} = $props();
</script>

{#if item.type === "user_message" || item.type === "assistant_message" || item.type === "reasoning_summary"}
	<ConversationMessage
		{image_sources}
		{item}
		{message_streaming}
		{onimagevisibilitychange}
		{steering_pending}
		{trailing}
	/>
{:else if item.type === "work_session"}
	<ConversationWorkSession {item} />
{:else if item.type === "activity"}
	<ConversationActivity {item} {trailing} />
{:else if item.type === "change_set" || item.type === "file_change"}
	<ConversationChange {item} />
{:else if item.type === "approval"}
	<ConversationApproval {item} {onapproval} />
{:else if item.type === "plan"}
	<!-- Plans are projected into the shell-owned Checklist, never the transcript. -->
{:else if item.type === "native_event"}
	<!-- Native diagnostics render only through ConversationTrace's visibility policy. -->
{:else if item.type === "question"}
	<ConversationPrompt {item} {onquestion} />
{:else if item.type === "usage_interruption"}
	<ConversationUsageInterruptionCard
		interruption={item.interruption}
		onresolve={onusageinterruptionresolve}
	/>
{:else}
	<ConversationStatus {item} {trailing} />
{/if}

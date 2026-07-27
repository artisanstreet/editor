<script lang="ts">
	import type { ConversationItem, ImageAttachmentReference } from "@artisan/protocol";
	import type { Effect } from "effect";
	import ConversationActivity from "./conversation-activity.sv";
	import ConversationApproval from "./conversation-approval.sv";
	import ConversationChange from "./conversation-change.sv";
	import ConversationMessage from "./conversation-message.sv";
	import ConversationPrompt from "./conversation-prompt.sv";
	import ConversationStatus from "./conversation-status.sv";
	import ConversationWorkSession from "./conversation-work-session.sv";
	import type { Snippet } from "svelte";

	let {
		item,
		image_sources,
		onapproval,
		onimagevisibilitychange,
		onquestion,
		trailing,
	}: {
		item: ConversationItem;
		image_sources?: ReadonlyMap<string, string>;
		onapproval?: (
			approval_id: string,
			approved: boolean,
		) => Effect.Effect<void, { readonly message: string }>;
		onimagevisibilitychange?: (
			attachments: ReadonlyArray<ImageAttachmentReference>,
			visible: boolean,
		) => void;
		onquestion?: (question_id: string, answer: string) => void;
		trailing?: Snippet;
	} = $props();
</script>

{#if item.type === "user_message" || item.type === "assistant_message" || item.type === "reasoning_summary"}
	<ConversationMessage {image_sources} {item} {onimagevisibilitychange} {trailing} />
{:else if item.type === "work_session"}
	<ConversationWorkSession {item} />
{:else if item.type === "activity"}
	<ConversationActivity {item} {trailing} />
{:else if item.type === "change_set" || item.type === "file_change"}
	<ConversationChange {item} />
{:else if item.type === "approval"}
	<ConversationApproval {item} {onapproval} />
{:else if item.type === "plan" || item.type === "question"}
	<ConversationPrompt {item} {onquestion} />
{:else}
	<ConversationStatus {item} {trailing} />
{/if}

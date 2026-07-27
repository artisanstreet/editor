<script lang="ts">
	import type {
		ConversationSnapshot,
		ImageAttachmentReference,
		ThreadSessionPolicy,
	} from "@artisan/protocol";
	import type { Effect } from "effect";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import {
		MakeConversationRenderBlocks,
		MakeConversationViewState,
		type ConversationRenderBlock,
	} from "$lib/conversation/store";
	import ConversationChangesCard from "./conversation-changes-card.sv";
	import ConversationItem from "./conversation-item.sv";
	import ConversationTrace from "./conversation-trace.sv";
	import ConversationTurnFooter from "./conversation-turn-footer.sv";
	import ConversationWorkSession from "./conversation-work-session.sv";
	import ThreadComposer from "./thread-composer.sv";

	let {
		disabled = false,
		image_sources,
		onapproval,
		onpolicychange,
		onquestion,
		onimagevisibilitychange,
		onsubmit,
		policy,
		snapshot,
	}: {
		disabled?: boolean;
		image_sources?: ReadonlyMap<string, string>;
		onapproval?: (approval_id: string, approved: boolean) => void;
		onpolicychange?: (policy: ThreadSessionPolicy) => void;
		onquestion?: (question_id: string, answer: string) => void;
		onimagevisibilitychange?: (
			attachments: ReadonlyArray<ImageAttachmentReference>,
			visible: boolean,
		) => void;
		onsubmit?: (submission: ComposerSubmission) => Effect.Effect<void, { readonly message: string }>;
		policy?: ThreadSessionPolicy;
		snapshot: ConversationSnapshot;
	} = $props();
	const view = $derived(MakeConversationViewState(snapshot));
	const render_blocks = $derived(
		view._tag === "applied" ? MakeConversationRenderBlocks(view.state) : [],
	);

	const render_groups = $derived.by(() => {
		const groups = new Map<
			string,
			{ blocks: Array<ConversationRenderBlock>; turn_id: string }
		>();

		for (const block of render_blocks) {
			const { turn_id } = block;
			const group = groups.get(turn_id);
			if (group === undefined) {
				groups.set(turn_id, { blocks: [block], turn_id });
			} else {
				group.blocks.push(block);
			}
		}

		return [...groups.values()];
	});
</script>

<main class="relative h-full min-h-0 overflow-hidden" aria-label="Thread workspace">
	<ScrollArea class="thread-transcript h-full min-h-0" scrollbarYClasses="hidden">
		<div class="mx-auto flex w-full max-w-3xl flex-col gap-8 px-6 pt-10 pb-48">
			{#if view._tag === "applied"}
				{#each render_groups as render_group (render_group.turn_id)}
					<section class="turn-hover-region group/turn relative flex flex-col gap-8">
						{#each render_group.blocks as block (block.id)}
							{#if block.type === "item"}
								<ConversationItem
									{image_sources}
									item={block.item}
									{onapproval}
									{onimagevisibilitychange}
									{onquestion}
								/>
							{:else if block.type === "work_group"}
								<ConversationWorkSession
									duration_kind={block.duration_kind}
									item={block.session}
								>
									{#snippet details()}
										<ConversationTrace items={block.details} />
									{/snippet}
								</ConversationWorkSession>
							{:else if block.type === "changes"}
								<ConversationChangesCard
									change_sets={block.change_sets}
									files={block.files}
								/>
							{:else}
								<ConversationTurnFooter
									settled_at={block.settled_at}
									text={block.text}
								/>
							{/if}
						{/each}
					</section>
				{/each}
			{:else}
				<p class="text-sm text-muted-foreground">Conversation history needs to be refreshed.</p>
			{/if}
		</div>
	</ScrollArea>

	<ThreadComposer {disabled} {onpolicychange} {onsubmit} {policy} />
</main>

<style>
	.turn-hover-region::after {
		position: absolute;
		top: 100%;
		left: 0;
		width: 100%;
		height: 2rem;
		content: "";
	}

	:global(.thread-transcript [data-slot="scroll-area-viewport"]) {
		-webkit-mask-image: linear-gradient(
			to bottom,
			transparent,
			black 16px,
			black calc(100% - 16px),
			transparent
		);
		mask-image: linear-gradient(
			to bottom,
			transparent,
			black 16px,
			black calc(100% - 16px),
			transparent
		);
		overscroll-behavior: contain;
	}
</style>

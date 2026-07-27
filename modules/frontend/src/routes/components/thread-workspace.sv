<script lang="ts">
	import type {
		ConversationSnapshot,
		ImageAttachmentReference,
		ThreadSessionPolicy,
	} from "@artisan/protocol";
	import { Effect, Option } from "effect";
	import { onDestroy, onMount, tick } from "svelte";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import type { ThreadMessageSubmissionOutcome } from "$lib/thread-interaction/commands";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { latest_active_activity_label } from "$lib/conversation/activity-status";
	import {
		ConversationAlignedScrollTop,
		ConversationBaseEndSpacePixels,
		ConversationBottomScrollTop,
		ConversationEndSpaceHeight,
		ConversationUserMessageIds,
		NewestConversationUserMessage,
	} from "$lib/conversation/scroll-position";
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
		onsubmit?: (
			submission: ComposerSubmission,
		) => Effect.Effect<
			ThreadMessageSubmissionOutcome,
			{ readonly message: string }
		>;
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

	let viewport = $state<HTMLElement | null>(null);
	let transcript_content = $state<HTMLElement | null>(null);
	let end_space = $state<HTMLElement | null>(null);
	let end_space_height = $state(ConversationBaseEndSpacePixels);
	let pending_user_message_ids = $state.raw<ReadonlySet<string> | undefined>();
	let anchored_user_item_id = $state<string | undefined>();
	let anchor_layout_frame = 0;
	let smooth_anchor_pending = false;
	let destroyed = false;

	const FindConversationItem = (item_id: string) =>
		[...(viewport?.querySelectorAll<HTMLElement>("[data-conversation-item-id]") ?? [])]
			.find((element) => element.dataset.conversationItemId === item_id);

	const UpdateAnchorLayout = async (smooth: boolean) => {
		await tick();
		if (destroyed || viewport === null || end_space === null) return;
		const item_id = anchored_user_item_id;
		if (item_id === undefined) return;
		const item = FindConversationItem(item_id);
		if (item === undefined) return;

		const item_bounds = item.getBoundingClientRect();
		const end_space_bounds = end_space.getBoundingClientRect();
		const next_end_space_height = ConversationEndSpaceHeight(
			viewport.clientHeight,
			item_bounds.top,
			end_space_bounds.top,
		);
		if (next_end_space_height !== end_space_height) {
			end_space_height = next_end_space_height;
			await tick();
		}
		if (destroyed || !smooth || viewport === null) return;

		const current_item = FindConversationItem(item_id);
		if (current_item === undefined) return;
		viewport.scrollTo({
			behavior: globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches
				? "auto"
				: "smooth",
			top: ConversationAlignedScrollTop(
				viewport.scrollTop,
				viewport.getBoundingClientRect().top,
				current_item.getBoundingClientRect().top,
			),
		});
	};

	const ScheduleAnchorLayout = (smooth = false) => {
		if (anchored_user_item_id === undefined || destroyed) return;
		smooth_anchor_pending ||= smooth;
		cancelAnimationFrame(anchor_layout_frame);
		anchor_layout_frame = requestAnimationFrame(() => {
			const should_smooth = smooth_anchor_pending;
			smooth_anchor_pending = false;
			void UpdateAnchorLayout(should_smooth);
		});
	};

	const SubmitMessage = (submission: ComposerSubmission) => {
		const submit = onsubmit;
		if (submit === undefined) return Effect.void;
		const previous_ids = ConversationUserMessageIds(snapshot.items);

		return submit(submission).pipe(
			Effect.tap((outcome) =>
				outcome.expects_user_message
					? Effect.sync(() => {
							pending_user_message_ids = previous_ids;
						})
					: Effect.void,
			),
		);
	};

	const PositionLoadedThread = async () => {
		await tick();
		if (destroyed || viewport === null) return;
		viewport.scrollTop = ConversationBottomScrollTop(
			viewport.scrollHeight,
			viewport.clientHeight,
		);
	};

	onMount(() => {
		void PositionLoadedThread();
	});

	onDestroy(() => {
		destroyed = true;
		cancelAnimationFrame(anchor_layout_frame);
	});

	$effect(() => {
		const current_items = snapshot.items;
		const previous_ids = pending_user_message_ids;
		if (previous_ids !== undefined) {
			const item_id = Option.getOrUndefined(
				NewestConversationUserMessage(current_items, previous_ids),
			);
			if (item_id !== undefined) {
				pending_user_message_ids = undefined;
				anchored_user_item_id = item_id;
				ScheduleAnchorLayout(true);
				return;
			}
		}
		if (anchored_user_item_id !== undefined) {
			ScheduleAnchorLayout();
		}
	});

	$effect(() => {
		if (
			transcript_content === null ||
			viewport === null ||
			!("ResizeObserver" in globalThis)
		)
			return;
		const observer = new ResizeObserver(() => ScheduleAnchorLayout());
		observer.observe(transcript_content);
		observer.observe(viewport);

		return () => observer.disconnect();
	});
</script>

<main class="relative h-full min-h-0 overflow-hidden" aria-label="Thread workspace">
	<ScrollArea
		bind:viewportRef={viewport}
		class="thread-transcript h-full min-h-0"
		scrollbarYClasses="hidden"
	>
		<div bind:this={transcript_content} class="mx-auto w-full max-w-3xl px-6 pt-10">
			<div class="flex flex-col gap-8">
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
										activity_label={latest_active_activity_label(block.details)}
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
					<p class="text-sm text-muted-foreground">
						Conversation history needs to be refreshed.
					</p>
				{/if}
			</div>
			<div
				bind:this={end_space}
				aria-hidden="true"
				style:height={`${end_space_height}px`}
			></div>
		</div>
	</ScrollArea>

	<ThreadComposer
		{disabled}
		{onpolicychange}
		onsubmit={onsubmit === undefined ? undefined : SubmitMessage}
		{policy}
	/>
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

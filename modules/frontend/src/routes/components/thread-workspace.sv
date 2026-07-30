<script lang="ts" effect>
	import type {
		ConversationSnapshot,
		ImageAttachmentReference,
		ThreadSessionPolicy,
	} from "@artisan/protocol";
	import { Effect, Option, Queue } from "effect";
	import { tick } from "svelte";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import type { ThreadMessageSubmissionOutcome } from "$lib/thread-interaction/commands";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { conversation_work_is_live } from "$lib/conversation/activity-status";
	import {
		ConversationAlignedScrollTop,
		ConversationBaseEndSpacePixels,
		ConversationBottomScrollTop,
		ConversationEndSpaceHeight,
		ConversationUserMessageWithSourceReference,
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
		onabort,
		onapproval,
		onpolicychange,
		onquestion,
		onimagevisibilitychange,
		onsubmit,
		policy,
		run_active = false,
		snapshot,
	}: {
		disabled?: boolean;
		image_sources?: ReadonlyMap<string, string>;
		onabort?: () => Effect.Effect<unknown, { readonly message: string }>;
		onapproval?: (
			approval_id: string,
			approved: boolean,
		) => Effect.Effect<void, { readonly message: string }>;
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
		run_active?: boolean;
		snapshot: ConversationSnapshot;
	} = $props();
	const view = $derived(MakeConversationViewState(snapshot));
	/** A settled thread may switch engines; only an in-flight run owns the current engine. */
	const engine_locked = $derived(run_active);
	type ConversationItemBlock = Extract<ConversationRenderBlock, { type: "item" }>;

	const block_is_resolved_approval = (
		block: ConversationRenderBlock,
	): block is ConversationItemBlock =>
		block.type === "item" &&
		block.item.type === "approval" &&
		block.item.state !== "requested";

	const fold_resolved_approvals_into_work = (
		blocks: ReadonlyArray<ConversationRenderBlock>,
	): ReadonlyArray<ConversationRenderBlock> => {
		const worked_turns = new Set(
			blocks
				.filter(
					(block) => block.type === "work_group" && block.duration_kind === "worked",
				)
				.map((block) => block.turn_id),
		);
		const approvals_by_turn = new Map<string, Array<ConversationItemBlock["item"]>>();

		for (const block of blocks) {
			if (!block_is_resolved_approval(block) || !worked_turns.has(block.turn_id)) continue;
			const approvals = approvals_by_turn.get(block.turn_id) ?? [];
			approvals.push(block.item);
			approvals_by_turn.set(block.turn_id, approvals);
		}

		return blocks.flatMap((block): ReadonlyArray<ConversationRenderBlock> => {
			if (block_is_resolved_approval(block) && worked_turns.has(block.turn_id)) return [];
			if (block.type !== "work_group" || block.duration_kind !== "worked") return [block];

			const approvals = approvals_by_turn.get(block.turn_id) ?? [];
			return [
				{
					...block,
					details: [...block.details, ...approvals].sort(
						(left, right) =>
							left.ordinal - right.ordinal || left.id.localeCompare(right.id),
					),
				},
			];
		});
	};

	const render_blocks = $derived(
		view._tag === "applied"
			? fold_resolved_approvals_into_work(MakeConversationRenderBlocks(view.state))
			: [],
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
	let pending_user_message_reference = $state<string | undefined>();
	let anchored_user_item_id = $state<string | undefined>();
	let anchor_layout_frame = 0;
	let smooth_anchor_pending = false;
	const anchor_layout_requests = yield* Queue.dropping<boolean>(1);
	const position_requests = yield* Queue.dropping<void>(1);

	const FindConversationItem = (item_id: string) =>
		[...(viewport?.querySelectorAll<HTMLElement>("[data-conversation-item-id]") ?? [])]
			.find((element) => element.dataset.conversationItemId === item_id);

	const UpdateAnchorLayout = (smooth: boolean) => Effect.gen(function* () {
		yield* Effect.tryPromise(() => tick()).pipe(Effect.ignore);
		if (viewport === null || end_space === null) return;
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
			yield* Effect.tryPromise(() => tick()).pipe(Effect.ignore);
		}
		if (!smooth || viewport === null) return;

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
	});

	const ScheduleAnchorLayout = (smooth = false) => {
		if (anchored_user_item_id === undefined) return;
		smooth_anchor_pending ||= smooth;
		cancelAnimationFrame(anchor_layout_frame);
		anchor_layout_frame = requestAnimationFrame(() => {
			const should_smooth = smooth_anchor_pending;
			smooth_anchor_pending = false;
			Queue.offerUnsafe(anchor_layout_requests, should_smooth);
		});
	};
	yield* Queue.take(anchor_layout_requests).pipe(
		Effect.flatMap(UpdateAnchorLayout),
		Effect.forever,
		Effect.forkScoped,
	);

	const SubmitMessage = (submission: ComposerSubmission) => {
		const submit = onsubmit;
		if (submit === undefined) return Effect.void;
		pending_user_message_reference = undefined;
		const ClearPendingUserMessage = Effect.sync(() => {
			pending_user_message_reference = undefined;
		});

		return submit(submission).pipe(
			Effect.tap((outcome) =>
				Effect.sync(() => {
					pending_user_message_reference =
						outcome.user_message_reference;
				}),
			),
			Effect.tapError(() => ClearPendingUserMessage),
		);
	};

	const PositionLoadedThread = Effect.gen(function* () {
		yield* Effect.tryPromise(() => tick()).pipe(Effect.ignore);
		if (viewport === null) return;
		viewport.scrollTop = ConversationBottomScrollTop(
			viewport.scrollHeight,
			viewport.clientHeight,
		);
	});
	yield* Queue.take(position_requests).pipe(
		Effect.flatMap(() => PositionLoadedThread),
		Effect.forever,
		Effect.forkScoped,
	);
	yield* Effect.addFinalizer(() => Effect.sync(() => cancelAnimationFrame(anchor_layout_frame)));

	$effect(() => {
		if (viewport === null) return;
		Queue.offerUnsafe(position_requests, undefined);
	});

	$effect(() => {
		const current_items = snapshot.items;
		const source_reference = pending_user_message_reference;
		if (source_reference !== undefined) {
			const item_id = Option.getOrUndefined(
				ConversationUserMessageWithSourceReference(
					current_items,
					source_reference,
				),
			);
			if (item_id !== undefined) {
				pending_user_message_reference = undefined;
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
										duration_kind={block.duration_kind}
										has_live_detail={conversation_work_is_live(block.details)}
										item={block.session}
										transition={block.transition}
									>
										{#snippet details()}
											<ConversationTrace
												failed={block.session.status === "failed" ||
													block.session.status === "cancelled"}
												items={block.details}
												work_active={block.session.ended_at === undefined}
											/>
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
		{engine_locked}
		{onabort}
		{onpolicychange}
		onsubmit={onsubmit === undefined ? undefined : SubmitMessage}
		{policy}
		{run_active}
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

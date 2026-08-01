<script lang="ts" effect>
	import type {
		ConversationSnapshot,
		ImageAttachmentReference,
		SurfaceUsageAggregate,
		ThreadSessionPolicy,
	} from "@artisan/protocol";
	import { Effect, Option, Queue } from "effect";
	import { tick } from "svelte";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import { RunBrowserDom } from "$lib/browser/dom";
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
		context_usage,
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
		context_usage?: SurfaceUsageAggregate;
		disabled?: boolean;
		image_sources?: ReadonlyMap<string, string>;
		onabort?: () => Effect.Effect<unknown, { readonly message: string }>;
		onapproval?: (
			approval_id: string,
			approved: boolean,
		) => Effect.Effect<void, { readonly message: string }>;
		onpolicychange?: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<ThreadSessionPolicy, { readonly message: string }>;
		onquestion?: (
			question_id: string,
			answer: string,
		) => Effect.Effect<void, { readonly message: string }>;
		onimagevisibilitychange?: (
			attachments: ReadonlyArray<ImageAttachmentReference>,
			visible: boolean,
		) => Effect.Effect<void>;
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
	let anchor_layout_revision = $state(0);
	let anchor_layout_smooth = $state(false);
	const anchor_layout_requests = yield* Queue.unbounded<
		| { readonly _tag: "request"; readonly smooth: boolean }
		| { readonly _tag: "flush"; readonly smooth: boolean }
	>();

	const FindConversationItem = (item_id: string) =>
		Effect.gen(function* () {
			return yield* RunBrowserDom(() =>
				[...(viewport?.querySelectorAll<HTMLElement>("[data-conversation-item-id]") ?? [])]
					.find((element) => element.dataset.conversationItemId === item_id),
			);
		});

	const UpdateAnchorLayout = (smooth: boolean) => Effect.gen(function* () {
		yield* Effect.promise(() => tick());
		if (viewport === null || end_space === null) return;
		const item_id = anchored_user_item_id;
		if (item_id === undefined) return;
		const item = yield* FindConversationItem(item_id);
		if (item === undefined) return;

		const { end_space_bounds, item_bounds, viewport_height } = yield* RunBrowserDom(() => ({
			end_space_bounds: end_space.getBoundingClientRect(),
			item_bounds: item.getBoundingClientRect(),
			viewport_height: viewport.clientHeight,
		}));
		const next_end_space_height = ConversationEndSpaceHeight(
			viewport_height,
			item_bounds.top,
			end_space_bounds.top,
		);
		if (next_end_space_height !== end_space_height) {
			end_space_height = next_end_space_height;
			yield* Effect.promise(() => tick());
		}
		if (!smooth || viewport === null) return;

		const current_item = yield* FindConversationItem(item_id);
		if (current_item === undefined) return;
		yield* RunBrowserDom(() => {
			viewport.scrollTo({
				behavior: globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth",
				top: ConversationAlignedScrollTop(
					viewport.scrollTop,
					viewport.getBoundingClientRect().top,
					current_item.getBoundingClientRect().top,
				),
			});
		});
	});

	const ScheduleAnchorLayout = Effect.gen(function* () {
		while (true) {
			const request = yield* Queue.take(anchor_layout_requests);
			if (request._tag === "flush") {
				anchor_layout_smooth = request.smooth;
				anchor_layout_revision += 1;
				continue;
			}
			if (anchored_user_item_id === undefined) continue;
			smooth_anchor_pending ||= request.smooth;
			yield* RunBrowserDom(() => {
				cancelAnimationFrame(anchor_layout_frame);
				anchor_layout_frame = requestAnimationFrame(() => {
					Queue.offerUnsafe(anchor_layout_requests, { _tag: "flush", smooth: smooth_anchor_pending });
					smooth_anchor_pending = false;
					anchor_layout_revision += 1;
				});
			});
		}
	});
	yield* ScheduleAnchorLayout.pipe(Effect.forkScoped);
	/** Visual geometry only: rAF coalesces observer churn before SER reruns this fiber. */
	if (anchor_layout_revision > 0) yield* UpdateAnchorLayout(anchor_layout_smooth);

	const SubmitMessage = (submission: ComposerSubmission) =>
		Effect.gen(function* () {
		const submit = onsubmit;
		if (submit === undefined) return;
		pending_user_message_reference = undefined;
		const ClearPendingUserMessage = Effect.gen(function* () {
			pending_user_message_reference = undefined;
		});

		yield* submit(submission).pipe(
			Effect.tap((outcome) =>
				Effect.gen(function* () {
					pending_user_message_reference =
						outcome.user_message_reference;
				}),
			),
			Effect.tapError(() =>
				Effect.gen(function* () {
					yield* ClearPendingUserMessage;
				}),
			),
		);
		});

	const PositionLoadedThread = Effect.gen(function* () {
		yield* Effect.promise(() => tick());
		if (viewport === null) return;
		yield* RunBrowserDom(() => {
			viewport.scrollTop = ConversationBottomScrollTop(viewport.scrollHeight, viewport.clientHeight);
		});
	});
	if (viewport !== null) yield* PositionLoadedThread;
	yield* Effect.addFinalizer(() =>
		Effect.gen(function* () {
			yield* RunBrowserDom(() => cancelAnimationFrame(anchor_layout_frame));
		}),
	);

	const ReconcileAnchor = Effect.gen(function* () {
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
				yield* Queue.offer(anchor_layout_requests, { _tag: "request", smooth: true });
				return;
			}
		}
		if (anchored_user_item_id !== undefined) {
			yield* Queue.offer(anchor_layout_requests, { _tag: "request", smooth: false });
		}
	});
	if (viewport !== null) yield* ReconcileAnchor;

	const transcript_size_observers = yield* MakeScopedAttachmentRunner(
		({ content, current_viewport }: { content: HTMLElement; current_viewport: HTMLElement }) =>
			Effect.gen(function* () {
				yield* Effect.acquireRelease(
					Effect.gen(function* () {
						return yield* RunBrowserDom(() => {
							const observer = new ResizeObserver(() => Queue.offerUnsafe(anchor_layout_requests, { _tag: "request", smooth: false }));
							observer.observe(content);
							observer.observe(current_viewport);
							return observer;
						});
					}),
					(observer) =>
						Effect.gen(function* () {
							yield* RunBrowserDom(() => observer.disconnect());
						}),
				);
				yield* Effect.never;
			}),
	);
	const SyncTranscriptSizeObserver = Effect.gen(function* () {
		const content = transcript_content;
		const current_viewport = viewport;
		const supports_resize_observer = yield* RunBrowserDom(() => "ResizeObserver" in globalThis);
		if (content === null || current_viewport === null || !supports_resize_observer) {
			yield* transcript_size_observers.Release("transcript-size");
			return;
		}
		yield* transcript_size_observers.Replace("transcript-size", {
			content,
			current_viewport,
		});
	});
	yield* SyncTranscriptSizeObserver;
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
		{context_usage}
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

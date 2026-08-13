<script lang="ts" effect>
	import type {
		ConversationSnapshot,
		ImageAttachmentReference,
		SurfaceUsageAggregate,
		ThreadSessionPolicy,
		ThreadWorkItem,
	} from "@artisan/protocol";
	import { Effect, Option, Queue } from "effect";
	import { tick, untrack } from "svelte";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import { RunBrowserDom } from "$lib/browser/dom";
	import type { ThreadMessageSubmissionOutcome } from "$lib/thread-interaction/commands";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { Button } from "$lib/components/ui/button";
	import {
		conversation_reply_is_live,
		conversation_waiting_for_activity,
		work_session_run_authority,
	} from "$lib/conversation/activity-status";
	import {
		ConversationAlignedScrollTop,
		ConversationBaseEndSpacePixels,
		ConversationBottomScrollTop,
		ConversationIsFollowing,
		ConversationEndSpaceHeight,
		ConversationUserMessageWithSourceReference,
	} from "$lib/conversation/scroll-position";
	import {
		MakeConversationRenderWindow,
		type ConversationRenderBlock,
		type ConversationViewState,
	} from "$lib/conversation/store";
	import ConversationChangesCard from "./conversation-changes-card.svelte";
	import ConversationItem from "./conversation-item.svelte";
	import ConversationTrace from "./conversation-trace.svelte";
	import ConversationTurnFooter from "./conversation-turn-footer.svelte";
	import ConversationWorkSession from "./conversation-work-session.svelte";
	import ThreadComposer from "./thread-composer.svelte";

	let {
		active_run_id,
		active_run_status,
		context_usage,
		conversation_view_state,
		disabled = false,
		image_sources,
		onabort,
		onapproval,
		onnewthread,
		onpolicychange,
		onquestion,
		onimagevisibilitychange,
		onsubmit,
		policy,
		project_root_path,
		run_active = false,
		snapshot,
	}: {
		active_run_id?: string;
		/** The durable work item's status for `active_run_id`, when one exists. */
		active_run_status?: ThreadWorkItem["status"];
		context_usage?: SurfaceUsageAggregate;
		conversation_view_state?: ConversationViewState;
		disabled?: boolean;
		image_sources?: ReadonlyMap<string, string>;
		onabort?: () => Effect.Effect<unknown, { readonly message: string }>;
		onapproval?: (
			approval_id: string,
			approved: boolean,
		) => Effect.Effect<void, { readonly message: string }>;
		onnewthread?: (
			submission: ComposerSubmission,
		) => Effect.Effect<unknown, { readonly message: string }>;
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
		project_root_path?: string;
		run_active?: boolean;
		snapshot: ConversationSnapshot;
	} = $props();
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

	const ConversationTurnPageSize = 24;
	let older_render_group_count = $state(0);
	let loading_older_turns = $state(false);
	const render_window = $derived(
		conversation_view_state === undefined
			? { blocks: [], hidden_group_count: 0 }
			: MakeConversationRenderWindow(
					conversation_view_state,
					ConversationTurnPageSize,
					older_render_group_count,
				),
	);
	const render_blocks = $derived(fold_resolved_approvals_into_work(render_window.blocks));
	/**
	 * The transcript's freshest session, which the durable work item may not
	 * describe yet: run authority treats exactly that one as pending rather
	 * than settled while the work item catches up.
	 */
	const newest_session_run_id = $derived(
		snapshot.items.findLast((item) => item.type === "work_session")?.run_id,
	);

	const visible_render_groups = $derived.by(() => {
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
	const hidden_render_group_count = $derived(render_window.hidden_group_count);

	const ShowEarlierTurns = Effect.gen(function* () {
		if (loading_older_turns) return;
		loading_older_turns = true;
		yield* Effect.gen(function* () {
			const current_viewport = viewport;
			const previous_scroll_height =
				current_viewport === null
					? undefined
					: yield* RunBrowserDom(() => current_viewport.scrollHeight);
			older_render_group_count += ConversationTurnPageSize;
			if (current_viewport === null || previous_scroll_height === undefined) return;
			yield* Effect.promise(() => tick());
			yield* RunBrowserDom(() => {
				current_viewport.scrollTop += current_viewport.scrollHeight - previous_scroll_height;
			});
		}).pipe(
			Effect.ensuring(
				Effect.gen(function* () {
					loading_older_turns = false;
				}),
			),
		);
	});

	let viewport = $state<HTMLElement | null>(null);
	let transcript_content = $state<HTMLElement | null>(null);
	let end_space = $state<HTMLElement | null>(null);
	let end_space_height = $state(ConversationBaseEndSpacePixels);
	let pending_user_message_reference = $state<string | undefined>();
	let anchored_user_item_id = $state<string | undefined>();
	/**
	 * Whether new content should pull the viewport down with it. Derived from
	 * scroll position on every scroll, so the reader is never in a mode they did
	 * not put themselves in — scrolling away turns it off, returning to the
	 * bottom turns it back on.
	 */
	let following = $state(true);
	/**
	 * Set while the anchor animates a submitted turn into place. A smooth scroll
	 * emits scroll events the whole way down, and reading follow state out of
	 * those intermediate positions would let a frame that happens to pass near
	 * the bottom re-arm following mid-animation and yank the reader away from
	 * the turn being anchored.
	 */
	let anchor_scroll_active = $state(false);
	let anchor_scroll_generation = 0;
	let anchor_layout_frame = 0;
	let smooth_anchor_pending = false;
	let anchor_layout_revision = $state(0);
	let anchor_layout_smooth = $state(false);
	/**
	 * Named rather than written inline at the yield site: the SER transform
	 * collects the identifiers of a type argument as reactive dependencies, and
	 * the property keys of an inline object type have no runtime binding to
	 * collect. A type reference resolves to nothing and is correctly ignored.
	 */
	type AnchorLayoutRequest =
		| { readonly _tag: "request"; readonly smooth: boolean }
		| { readonly _tag: "flush"; readonly smooth: boolean };
	const anchor_layout_requests = yield* Queue.unbounded<AnchorLayoutRequest>();

	/** Reads follow state back from wherever the viewport actually settled. */
	const SyncFollowing = (element: HTMLElement) => {
		if (anchor_scroll_active) return;
		following = ConversationIsFollowing(
			element.scrollTop,
			element.scrollHeight,
			element.clientHeight,
		);
	};

	const release_anchor_scroll = (element: HTMLElement) => {
		anchor_scroll_generation += 1;
		anchor_scroll_active = false;
		SyncFollowing(element);
	};

	/**
	 * How far a single correction may be softened. Beyond about two lines the
	 * content would still be visibly travelling when the next one lands, so a
	 * large arrival — a settled code block, a card — takes its place at once
	 * rather than sliding a third of the viewport.
	 */
	const follow_glide_ceiling = 56;

	const reduced_motion = yield* RunBrowserDom(
		() => globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches,
	).pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
				return true;
			}),
		),
	);

	/**
	 * Turns a follow correction into a glide without touching the scroll again.
	 *
	 * The pin must stay instant: a smooth scroll emits intermediate positions
	 * that read as a reader who scrolled away, which is what used to switch
	 * following off mid-turn. So the scroll lands at once and the content is
	 * offset by exactly what the scroll moved, then transitioned back to zero —
	 * the reader sees a glide while `scrollTop` was correct the whole time.
	 *
	 * Each revealed word adds a fraction of a line and is absorbed invisibly.
	 * What this smooths is the line break, where a whole line height arrives in
	 * one frame and used to chop the text down mid-sentence.
	 */
	const GlideFollowCorrection = (content: HTMLElement, delta: number) => {
		if (delta <= 0 || reduced_motion) return;
		content.style.transition = "none";
		content.style.transform = `translateY(${Math.min(delta, follow_glide_ceiling)}px)`;
		/** Committing the start position is what makes the return animate at all. */
		void content.offsetHeight;
		content.style.transition = "transform var(--duration-fast) var(--ease-smooth-out)";
		content.style.transform = "translateY(0px)";
	};

	/**
	 * Guards smooth-scroll intermediate positions, but never relies exclusively
	 * on `scrollend`: no-movement, interrupted, and older runtimes can omit it.
	 * The generation fence makes a newer scroll own the one-second fallback.
	 */
	const ArmAnchorScroll = (element: HTMLElement, next_following: boolean) =>
		Effect.gen(function* () {
			const generation = (anchor_scroll_generation += 1);
			yield* Effect.gen(function* () {
				yield* Effect.sleep("1 second");
				if (generation === anchor_scroll_generation) release_anchor_scroll(element);
			}).pipe(Effect.forkScoped);
			following = next_following;
			anchor_scroll_active = true;
		});

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

		const { end_space_bounds, item_bounds, viewport_height, viewport_scroll_top, viewport_top } =
			yield* RunBrowserDom(() => ({
				end_space_bounds: end_space.getBoundingClientRect(),
				item_bounds: item.getBoundingClientRect(),
				viewport_height: viewport.clientHeight,
				viewport_scroll_top: viewport.scrollTop,
				viewport_top: viewport.getBoundingClientRect().top,
			}));
		const next_end_space_height = ConversationEndSpaceHeight(
			viewport_height,
			item_bounds.top,
			end_space_bounds.top,
		);
		/**
		 * The reserved space at its floor is the designed handoff from the
		 * anchored reading position to following the tail — but the follow pin is
		 * gated on `following`, which anchoring switched off, so the handoff must
		 * re-arm it here. It keys on the floor state itself rather than the
		 * transition into it: a short viewport can clamp the very first
		 * measurement to the floor, and an edge that never fires would leave the
		 * run growing below the fold while the transcript looks frozen. The
		 * smooth pass is excluded because it is the anchor scroll itself, and
		 * only a reader still parked where the anchor put them is handed over;
		 * anyone who scrolled away chose their own position.
		 */
		if (
			!smooth &&
			next_end_space_height <= ConversationBaseEndSpacePixels &&
			!following &&
			!anchor_scroll_active &&
			Math.abs(
				ConversationAlignedScrollTop(viewport_scroll_top, viewport_top, item_bounds.top) -
					viewport_scroll_top,
			) <= 32
		) {
			following = true;
		}
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
		/** The anchor parks the reader at this turn's top, which is not the bottom. */
		yield* ArmAnchorScroll(viewport, false);
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
				/**
					 * The flush alone publishes the revision. Bumping it here too ran
					 * a pass against the previous flush's `smooth`, so a coalesced
					 * batch could perform its layout under a stale flag and leave the
					 * real one racing an in-flight run.
					 */
					anchor_layout_frame = requestAnimationFrame(() => {
					Queue.offerUnsafe(anchor_layout_requests, { _tag: "flush", smooth: smooth_anchor_pending });
					smooth_anchor_pending = false;
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
					pending_user_message_reference = outcome.user_message_reference;
					if (outcome.user_message_reference !== undefined) following = false;
				}),
			),
			Effect.tapError(() =>
				Effect.gen(function* () {
					yield* ClearPendingUserMessage;
				}),
			),
		);
		});

	/**
	 * Leaves an anchored turn and resumes the ordinary live-tail contract. The
	 * anchor's spacer is released before measuring the bottom, or the jump would
	 * land after a screenful of space that existed only to place the sent turn at
	 * the top.
	 */
	const JumpToLatest = Effect.gen(function* () {
		const current_viewport = viewport;
		if (current_viewport === null) return;
		pending_user_message_reference = undefined;
		anchored_user_item_id = undefined;
		end_space_height = ConversationBaseEndSpacePixels;
		yield* Effect.promise(() => tick());
		yield* RunBrowserDom(() => {
			current_viewport.scrollTo({
				behavior: globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches
					? "auto"
					: "smooth",
				top: ConversationBottomScrollTop(
					current_viewport.scrollHeight,
					current_viewport.clientHeight,
				),
			});
		});
		yield* ArmAnchorScroll(current_viewport, true);
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

	/**
	 * Takes the transcript and the pending send as arguments: SER derives a
	 * program's inputs from the yielded expression, so a new message read only
	 * inside this body would never rerun the anchor and the bubble it belongs
	 * to would stay wherever the transcript happened to leave it.
	 */
	const ReconcileAnchor = (
		current_items: ConversationSnapshot["items"],
		source_reference: string | undefined,
	) =>
		Effect.gen(function* () {
		if (source_reference !== undefined) {
			const item_id = Option.getOrUndefined(
				ConversationUserMessageWithSourceReference(
					current_items,
					source_reference,
				),
			);
			if (item_id !== undefined) {
				/** The same send resolving twice must not scroll the reader twice. */
				if (anchored_user_item_id === item_id) return;
				anchored_user_item_id = item_id;
				/**
				 * Forked to the component scope rather than run inline, and the
				 * pending reference cleared only after.
				 *
				 * That reference is a dependency of this very statement, so writing
				 * it re-runs this program and interrupts whatever it was doing. An
				 * inline anchor pass yields for a tick before it can measure
				 * anything, which is a wide enough window to be interrupted every
				 * time — and the re-run, now carrying no reference, falls through to
				 * the relayout branch, which by design never scrolls. The send
				 * therefore resolved, anchored, and then quietly did nothing, on
				 * every thread long enough for the move to be visible at all.
				 */
				yield* UpdateAnchorLayout(true).pipe(Effect.forkScoped);
				pending_user_message_reference = undefined;
				return;
			}
		}
		if (anchored_user_item_id !== undefined) {
			yield* Queue.offer(anchor_layout_requests, { _tag: "request", smooth: false });
		}
	});
	if (viewport !== null) {
		yield* ReconcileAnchor(snapshot.items, pending_user_message_reference);
	}

	const transcript_size_observers = yield* MakeScopedAttachmentRunner(
		({ content, current_viewport }: { content: HTMLElement; current_viewport: HTMLElement }) =>
			Effect.gen(function* () {
				yield* Effect.acquireRelease(
					Effect.gen(function* () {
						return yield* RunBrowserDom(() => {
							const observer = new ResizeObserver(() => {
								Queue.offerUnsafe(anchor_layout_requests, { _tag: "request", smooth: false });
								/**
								 * Applied here rather than through the queue: the queue
								 * settles a frame later, which is long enough for growing
								 * content to show a visible slip before the correction
								 * lands.
								 *
								 * Instant, never smooth. Following a stream fires this on
								 * every revealed word, and a smooth scroll is an animation
								 * with a duration: each word retargets one already in
								 * flight, which reads as the transcript shivering, and the
								 * intermediate positions it emits are nowhere near the
								 * bottom — one of them lands in a scroll handler, reads as
								 * a reader who scrolled away, and following switches off
								 * mid-turn. Pinning outright has neither failure and is
								 * imperceptible at one word of growth per frame.
								 *
								 * A reserved end space outranks it. While that space is
								 * above its floor it is absorbing the answer's growth to
								 * hold the anchored turn at the top inset, which means the
								 * reader is already parked where they should be and the
								 * total height is not moving. Pinning as well makes the two
								 * fight once per revealed word: the pin scrolls down by the
								 * new line, then the space shrinks by that same line a frame
								 * later and the position snaps back — visible as the
								 * transcript flicking on every render. The space bottoming
								 * out is what hands following the tail over.
								 */
								if (
									!following ||
									anchor_scroll_active ||
									end_space_height > ConversationBaseEndSpacePixels
								)
									return;
								const settled_at = current_viewport.scrollTop;
								current_viewport.scrollTo({
									behavior: "auto",
									top: ConversationBottomScrollTop(
										current_viewport.scrollHeight,
										current_viewport.clientHeight,
									),
								});
								GlideFollowCorrection(content, current_viewport.scrollTop - settled_at);
							});
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

	/**
	 * Listeners rather than markup handlers: the viewport belongs to `ScrollArea`
	 * and is reached by binding, so there is no element here to put them on.
	 * Scoped so they release with the component.
	 */
	const follow_listeners = yield* MakeScopedAttachmentRunner(
		({ current_viewport }: { current_viewport: HTMLElement }) =>
			Effect.gen(function* () {
				yield* Effect.acquireRelease(
					RunBrowserDom(() => {
						const on_scroll = () => SyncFollowing(current_viewport);
						/** `scrollend` is what releases the anchor guard once its animation settles. */
						const on_scroll_end = () => release_anchor_scroll(current_viewport);
						current_viewport.addEventListener("scroll", on_scroll, { passive: true });
						current_viewport.addEventListener("scrollend", on_scroll_end, {
							passive: true,
						});
						return { on_scroll, on_scroll_end };
					}),
					(handlers) =>
						RunBrowserDom(() => {
							current_viewport.removeEventListener("scroll", handlers.on_scroll);
							current_viewport.removeEventListener("scrollend", handlers.on_scroll_end);
						}),
				);
				yield* Effect.never;
			}),
	);
	const SyncFollowListeners = Effect.gen(function* () {
		const current_viewport = viewport;
		if (current_viewport === null) {
			yield* follow_listeners.Release("follow");
			return;
		}
		yield* follow_listeners.Replace("follow", { current_viewport });
	});
	yield* SyncFollowListeners;
</script>


<main class="relative h-full min-h-0 overflow-hidden" aria-label="Thread workspace">
	<ScrollArea
		bind:viewportRef={viewport}
		class="thread-transcript h-full min-h-0"
		scrollbarYClasses="hidden"
	>
		<div bind:this={transcript_content} class="mx-auto w-full max-w-(--prose-width) px-6 pt-10">
			<div class="flex flex-col gap-8">
				{#if conversation_view_state !== undefined}
					{#if hidden_render_group_count > 0}
						<div class="flex justify-center pb-2">
							<Button
								variant="ghost"
								size="sm"
								disabled={loading_older_turns}
								onclick={yield* ShowEarlierTurns}
							>
								Show earlier turns ({hidden_render_group_count})
							</Button>
						</div>
					{/if}
					{#each visible_render_groups as render_group (render_group.turn_id)}
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
									{@const session_authority = work_session_run_authority({
										active_run_id,
										active_run_status,
										newest_session_run_id,
										session_run_id: block.session.run_id,
									})}
									<ConversationWorkSession
										duration_kind={block.duration_kind}
										engine_id={policy?.engine_id}
										has_details={block.details.length > 0}
										has_live_reply={conversation_reply_is_live(block.details)}
										item={block.session}
										run_authority={session_authority}
										transition={block.transition}
										waiting_for_activity={conversation_waiting_for_activity(block.details)}
									>
										{#snippet details()}
											<!-- Stopping is the user's own act, not a failure the trace must explain. -->
											<ConversationTrace
												failed={block.session.status === "failed"}
												items={block.details}
												work_active={block.session.ended_at === undefined &&
													session_authority === "active"}
											/>
										{/snippet}
									</ConversationWorkSession>
								{:else if block.type === "changes"}
									<ConversationChangesCard
										change_sets={block.change_sets}
										files={block.files}
										{project_root_path}
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
		draft_key={snapshot.thread_id}
		{engine_locked}
		{onabort}
		onjumptolatest={JumpToLatest}
		{onnewthread}
		{onpolicychange}
		onsubmit={onsubmit === undefined ? undefined : SubmitMessage}
		{policy}
		{run_active}
		show_jump_to_latest={!following && !anchor_scroll_active}
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

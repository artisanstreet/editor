<script lang="ts" effect>
	import type {
		ConversationSnapshot,
		ImageAttachmentReference,
		SurfaceUsageAggregate,
		ThreadSessionPolicy,
		ThreadWorkItem,
	} from "@artisan/protocol";
	import { Effect, Exit, Option, Queue, Scope, Stream } from "effect";
	import { tick, untrack } from "svelte";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import { RunBrowserDom } from "$lib/browser/dom";
	import type { ThreadMessageSubmissionOutcome } from "$lib/thread-interaction/commands";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { Button } from "$lib/components/ui/button";
	import {
		conversation_background_agent_names,
		conversation_progress_phase,
		conversation_reply_is_live,
		conversation_waiting_for_activity,
		work_session_is_settled,
	} from "$lib/conversation/activity-status";
	import {
		conversation_live_reasoning_summary,
		group_conversation_trace_blocks,
		strip_conversation_trace_reasoning,
		type ConversationTraceRenderBlock,
	} from "$lib/conversation/trace";
	import {
		ActiveConversationTurn,
		ConversationTurnMarkers,
		type ConversationTurnMarker,
	} from "$lib/conversation/turn-navigator";
	import { policy_reasoning_display } from "$lib/engine/reasoning-display";
	import {
		ConversationAlignedScrollTop,
		ConversationBaseEndSpacePixels,
		ConversationBottomScrollTop,
		ConversationIsFollowing,
		ConversationEndSpaceHeight,
		ConversationUserMessageWithSourceReference,
	} from "$lib/conversation/scroll-position";
	import {
		ConversationOlderGroupCountForItem,
		MakeParticipantConversationRenderWindow,
		type ConversationRenderBlock,
		type ConversationViewState,
	} from "$lib/conversation/store";
	import ConversationChangesCard from "./conversation-changes-card.svelte";
	import ConversationItem from "./conversation-item.svelte";
	import ConversationTurnNavigator from "./conversation-turn-navigator.svelte";
	import ConversationTrace from "./conversation-trace.svelte";
	import ConversationTurnFooter from "./conversation-turn-footer.svelte";
	import ConversationWorkSession from "./conversation-work-session.svelte";
	import ThreadComposer from "./thread-composer.svelte";
	import { ComposerContextWindowTokens } from "$lib/composer/send-readiness";
	import { ContextCompactionIsImminent } from "$lib/context-usage/auto-compaction";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";

	let {
		active_run_id,
		active_run_status,
		context_usage,
		conversation_view_state,
		disabled = false,
		first_submission_reference,
		image_sources,
		inspection,
		onreturntoroot,
		onabort,
		onapproval,
		onnewthread,
		onpolicychange,
		onquestion,
		onretry,
		onusageinterruptionresolve,
		onimagevisibilitychange,
		onsubmit,
		onwithdraw,
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
		/**
		 * The command id of a first message claimed from the draft route. That
		 * send never passes through this workspace's own submit path, so the
		 * anchor that places a sent turn at the top must be seeded from outside
		 * or a thread's very first turn is the one send that never anchors.
		 */
		first_submission_reference?: string;
		image_sources?: ReadonlyMap<string, string>;
		inspection?: { readonly agent_id: string; readonly display_name: string };
		/** The roster owns this Effect; the workspace only yields it at navigation events. */
		onreturntoroot?: Effect.Effect<void>;
		onabort?: () => Effect.Effect<unknown, { readonly message: string }>;
		onapproval?: (
			approval_id: string,
			approved: boolean,
		) => Effect.Effect<void, { readonly message: string }>;
		onnewthread?: (
			submission: ComposerSubmission,
		) => Effect.Effect<void, { readonly message: string }>;
		onpolicychange?: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<ThreadSessionPolicy, { readonly message: string }>;
		onquestion?: (
			question_id: string,
			answers: ReadonlyArray<string>,
		) => Effect.Effect<void, { readonly message: string }>;
		onretry?: (
			run_id: string,
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
		/** Recalls one queued steer by the send's own command id, while Forge still holds it. */
		onwithdraw?: (command_id: string) => Effect.Effect<void, { readonly message: string }>;
		policy?: ThreadSessionPolicy;
		project_root_path?: string;
		run_active?: boolean;
		snapshot: ConversationSnapshot;
	} = $props();
	/** A settled thread may switch engines; only an in-flight run owns the current engine. */
	const engine_locked = $derived(run_active);

	/**
	 * The catalog is read here, and not left to the composer that also reads it,
	 * because the window size answers a question about the transcript: whether
	 * the quiet run above is compacting. Fewer than half of recorded runs report
	 * `context_window_tokens` themselves, so the catalog fallback is what keeps
	 * that answer from being unavailable most of the time.
	 */
	const defaults_controller = yield* SessionDefaultsController;
	let defaults_state = $state.raw<SessionDefaultsState>(yield* defaults_controller.Current);
	/**
	 * Bound to a const rather than written inline: an inline handler makes this
	 * yield site read the very state it writes, which is the reactive loop that
	 * has taken the renderer down before.
	 */
	const ApplyDefaults = (next: SessionDefaultsState) =>
		Effect.gen(function* () {
			defaults_state = next;
		});
	yield* defaults_controller.Changes.pipe(
		Stream.runForEach(ApplyDefaults),
		Effect.forkScoped,
	);
	const awaiting_compaction = $derived(
		run_active === true &&
			ContextCompactionIsImminent(
				context_usage,
				ComposerContextWindowTokens(defaults_state.catalog, policy, context_usage),
			),
	);
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
			: MakeParticipantConversationRenderWindow(
					conversation_view_state,
					inspection?.agent_id,
					ConversationTurnPageSize,
					older_render_group_count,
				),
	);
	const render_blocks = $derived(
		group_conversation_trace_blocks(
			fold_resolved_approvals_into_work(render_window.blocks),
		),
	);
	/**
	 * What the active run's thinking line says. A model that streams raw
	 * chain-of-thought publishes no summary to say, so its turns keep the
	 * thinking verb throughout.
	 */
	const live_reasoning_summary = $derived(
		policy_reasoning_display(policy) === "trace"
			? undefined
			: conversation_live_reasoning_summary(render_blocks, active_run_id, run_active),
	);
	/**
	 * The transcript's freshest session, which the durable work item may not
	 * describe yet: run authority treats exactly that one as pending rather
	 * than settled while the work item catches up.
	 */
	const newest_session_run_id = $derived(
		snapshot.items.findLast((item) => item.type === "work_session")?.run_id,
	);

	const visible_render_groups = $derived.by(() => {
		const groups: Array<{
			blocks: Array<ConversationTraceRenderBlock>;
			segment_id: string;
			turn_id: string;
		}> = [];

		for (const block of render_blocks) {
			const { turn_id } = block;
			const group = groups.at(-1);
			if (group?.turn_id === turn_id) group.blocks.push(block);
			else
				groups.push({
					blocks: [block],
					/** A turn may legitimately reappear after an acknowledged steer. */
					segment_id: JSON.stringify([turn_id, block.id]),
					turn_id,
				});
		}

		return groups;
	});
	const hidden_render_group_count = $derived(render_window.hidden_group_count);
	const inspecting_agent = $derived(inspection !== undefined);
	let steering_pending = $state(false);
	const SetSteeringPending = (pending: boolean) => {
		steering_pending = pending;
	};
	const ReturnToRoot = Effect.gen(function* () {
		if (onreturntoroot !== undefined) yield* onreturntoroot;
	});
	const ReturnToRootOnEscape = (event: KeyboardEvent) =>
		Effect.gen(function* () {
			if (event.key === "Escape" && inspecting_agent) yield* ReturnToRoot;
		});

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
	/** Seeded with a claimed first submission so a new thread's opening turn anchors like any other send. */
	let pending_user_message_reference = $state<string | undefined>(first_submission_reference);
	let anchored_user_item_id = $state<string | undefined>();
	/**
	 * Whether new content should pull the viewport down with it. Derived from
	 * scroll position on every scroll, so the reader is never in a mode they did
	 * not put themselves in — scrolling away turns it off, returning to the
	 * bottom turns it back on. A seeded first submission starts it off, exactly
	 * as an ordinary send switches it off at acceptance, so the tail cannot pull
	 * the reader while that turn's anchor is still on its way.
	 */
	let following = $state(first_submission_reference === undefined);
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
	let anchor_layout_pending = false;
	let anchor_layout_pending_smooth = false;
	let anchor_layout_revision = $state(0);
	let anchor_layout_smooth = $state(false);
	/**
	 * Geometry state lives out of band, while every request retains a wake token
	 * until the layout worker observes it. A pending rAF cannot erase a later
	 * request because its pending state and wake are both preserved.
	 */
	const anchor_layout_wake = yield* Queue.unbounded<void>();
	/**
	 * A component-lifetime home for the anchor's own scroll work. A reactive
	 * statement's scope closes on every rerun, and the statement that anchors a
	 * sent turn reruns the moment the anchor writes state — so work forked with
	 * `forkScoped` from inside it lands in that run scope and is interrupted
	 * before it can scroll. Anything the anchor must finish outlives its
	 * originating rerun only by being forked in here.
	 */
	const anchor_scope = yield* Scope.make();
	yield* Effect.addFinalizer(() => Scope.close(anchor_scope, Exit.void));

	const MarkAnchorLayoutPending = (smooth: boolean) => {
		anchor_layout_pending = true;
		anchor_layout_pending_smooth ||= smooth;
	};

	const RequestAnchorLayout = (smooth: boolean) =>
		Effect.gen(function* () {
			if (anchored_user_item_id === undefined) return;
			MarkAnchorLayoutPending(smooth);
			yield* Queue.offer(anchor_layout_wake, undefined);
		});

	/** ResizeObserver is synchronous browser ingress, so it cannot yield. */
	const RequestAnchorLayoutUnsafe = (smooth: boolean) => {
		if (anchored_user_item_id === undefined) return;
		MarkAnchorLayoutPending(smooth);
		Queue.offerUnsafe(anchor_layout_wake, undefined);
	};

	/**
	 * True once this mount has placed the reader at the latest content. The
	 * viewport binds before the view state arrives, so placement waits for the
	 * rendered transcript and runs exactly once.
	 */
	let positioned = $state(false);

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
			/** Forked into the component scope: the fallback must outlive the statement rerun that armed it. */
			yield* Effect.gen(function* () {
				yield* Effect.sleep("1 second");
				if (generation === anchor_scroll_generation) release_anchor_scroll(element);
			}).pipe(Effect.forkIn(anchor_scope));
			following = next_following;
			anchor_scroll_active = true;
		});

	/**
	 * The transcript's own map, down the right edge of the card.
	 *
	 * Position is read from where the turns actually sit rather than tracked as
	 * the reader moves: scrolling is the only thing that changes it, the marks
	 * are few, and a measurement taken on the spot cannot drift out of step with
	 * a transcript that grows and reflows underneath it.
	 */
	const turn_markers = $derived(ConversationTurnMarkers(snapshot.items));
	let active_turn_id = $state<string | undefined>(undefined);

	/** Synchronous scroll ingress, so it measures and assigns without yielding. */
	const SyncActiveTurn = () => {
		const current_viewport = viewport;
		if (current_viewport === null || turn_markers.length === 0) return;
		const viewport_top = current_viewport.getBoundingClientRect().top;
		const offsets = turn_markers.flatMap((marker) => {
			const element = current_viewport.querySelector<HTMLElement>(
				`[data-conversation-item-id="${CSS.escape(marker.id)}"]`,
			);
			return element === null
				? []
				: [{ id: marker.id, top: element.getBoundingClientRect().top - viewport_top }];
		});
		active_turn_id = ActiveConversationTurn(offsets);
	};

	const FindConversationItem = (item_id: string) =>
		Effect.gen(function* () {
			return yield* RunBrowserDom(() =>
				[...(viewport?.querySelectorAll<HTMLElement>("[data-conversation-item-id]") ?? [])]
					.find((element) => element.dataset.conversationItemId === item_id),
			);
		});

	const SelectTurn = (marker: ConversationTurnMarker) =>
		Effect.gen(function* () {
			let item = yield* FindConversationItem(marker.id);
			if (item === undefined && conversation_view_state !== undefined) {
				const required_older_groups = ConversationOlderGroupCountForItem(
					conversation_view_state,
					inspection?.agent_id,
					ConversationTurnPageSize,
					marker.id,
				);
				if (
					required_older_groups !== undefined &&
					required_older_groups > older_render_group_count
				) {
					older_render_group_count = required_older_groups;
					yield* Effect.promise(() => tick());
					item = yield* FindConversationItem(marker.id);
				}
			}
			if (item === undefined) return;
			/**
			 * Jumping is the reader taking control of where they are, so it also
			 * stops the tail from pulling them back off the turn they asked for.
			 */
			following = false;
			yield* RunBrowserDom(() => {
				item.scrollIntoView({ behavior: "smooth", block: "start" });
			});
			active_turn_id = marker.id;
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
			yield* Queue.take(anchor_layout_wake);
			yield* RunBrowserDom(() => {
				cancelAnimationFrame(anchor_layout_frame);
				anchor_layout_frame = requestAnimationFrame(() => {
					anchor_layout_frame = 0;
					if (!anchor_layout_pending) return;
					/** Snapshot before publishing: a later request leaves fresh pending state and its own wake token. */
					const smooth = anchor_layout_pending_smooth;
					anchor_layout_pending = false;
					anchor_layout_pending_smooth = false;
					anchor_layout_smooth = smooth;
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

		/**
		 * Returned, not just awaited: the composer stages its queued lip and the
		 * "Steering" label from the outcome's settlement effects, and a swallowed
		 * outcome left both to guesswork — the label rose at submit and nothing
		 * ever confirmed the steer.
		 */
		return yield* submit(submission).pipe(
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

	/**
	 * Every thread opens at its latest content. This is an assignment rather
	 * than a scroll: a thread being entered has no position to animate away
	 * from, and animating would show a journey through history nobody requested.
	 *
	 * Takes the view state as its argument for the same reason ReconcileAnchor
	 * takes the transcript: the statement below must rerun when it arrives.
	 * The viewport binds while the view state is still undefined and the
	 * transcript is rendered from the view state, so positioning at bind time
	 * would measure an empty scroller and land before the content exists.
	 */
	const PositionLoadedThread = (view_state: ConversationViewState | undefined) =>
		Effect.gen(function* () {
			if (view_state === undefined) return;
			yield* Effect.promise(() => tick());
			if (viewport === null || positioned) return;
			yield* RunBrowserDom(() => {
				viewport.scrollTop = ConversationBottomScrollTop(
					viewport.scrollHeight,
					viewport.clientHeight,
				);
			});
			positioned = true;
		});
	if (viewport !== null && !positioned) {
		yield* PositionLoadedThread(conversation_view_state);
	}
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
				 * Forked into `anchor_scope` rather than run inline or `forkScoped`,
				 * and the pending reference cleared only after.
				 *
				 * That reference is a dependency of this very statement, so writing
				 * it re-runs this program and interrupts whatever it was doing. An
				 * inline anchor pass yields for a tick before it can measure
				 * anything, which is a wide enough window to be interrupted every
				 * time — and the re-run, now carrying no reference, falls through to
				 * the relayout branch, which by design never scrolls. `forkScoped`
				 * has the same fate spelled differently: it forks into this run's
				 * scope, which the rerun closes. The send then resolved, anchored,
				 * grew the end space through the relayout branch, and quietly never
				 * scrolled. Only a component-lifetime scope survives the rerun.
				 */
				yield* UpdateAnchorLayout(true).pipe(Effect.forkIn(anchor_scope));
				pending_user_message_reference = undefined;
				return;
			}
		}
		if (anchored_user_item_id !== undefined) {
			yield* RequestAnchorLayout(false);
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
								RequestAnchorLayoutUnsafe(false);
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
						const on_scroll = () => {
							SyncFollowing(current_viewport);
							SyncActiveTurn();
						};
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

<svelte:window onkeydown={yield* ReturnToRootOnEscape(event)} />

<main class="relative h-full min-h-0 overflow-hidden" aria-label={inspecting_agent ? "Agent conversation" : "Thread workspace"}>
	<ConversationTurnNavigator
		active_id={active_turn_id}
		markers={turn_markers}
		onselect={SelectTurn}
	/>
	<!--
		The fade belongs to the frame, not to the element that scrolls. On the
		viewport it masked the scroller itself, and a masked scroller is composited
		on its own layer whose hit region no longer matches the box the wheel is
		tested against — the transcript stopped taking wheel input while every
		unmasked scroller in the shell kept working. The root paints the same
		gradient over the same box without touching what scrolls underneath it.
	-->
	<ScrollArea
		bind:viewportRef={viewport}
		class="transcript-fade h-full min-h-0"
		scrollbarYClasses="hidden"
		viewportClasses="overscroll-contain"
	>
		<div bind:this={transcript_content} class="prose-column w-full max-w-(--prose-width) px-6 pt-10">
			<div class="flex flex-col gap-8">
				{#if inspection !== undefined}
					<header class="flex items-center gap-2 text-sm text-muted-foreground">
						<button type="button" class="text-foreground hover:underline" onclick={yield* ReturnToRoot}>Back</button>
						<span>Viewing {inspection.display_name}'s conversation</span>
					</header>
				{/if}
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
					{#if visible_render_groups.length === 0 && inspection !== undefined}
						<p class="text-sm text-muted-foreground">No conversation has been exposed yet.</p>
					{/if}
					{#each visible_render_groups as render_group (render_group.segment_id)}
						<!-- The pad below each turn keeps its hover alive across the gap to the next. -->
						<section
							class="group/turn relative flex flex-col gap-[1lh] after:absolute after:top-full after:left-0 after:h-8 after:w-full after:content-['']"
						>
							{#each render_group.blocks as block (block.id)}
								{#if block.type === "trace_group"}
									{@const visible_trace_items = strip_conversation_trace_reasoning(
										block.items,
									)}
									<!--
										Post-steer trace material keeps the same policy boundary: activity
										stays collapsible, while diagnostics remain hidden unless the trace
										explicitly exposes them.
									-->
									<ConversationTrace
										items={visible_trace_items}
										work_active={run_active &&
											block.items.some((item) => item.run_id === active_run_id)}
									/>
								{:else if block.type === "item"}
									<ConversationItem
										{image_sources}
										item={block.item}
										{onapproval}
										{onimagevisibilitychange}
										{onquestion}
										{onusageinterruptionresolve}
									/>
								{:else if block.type === "work_group"}
									{@const session_settled = work_session_is_settled(block.session.status)}
									{@const visible_details = strip_conversation_trace_reasoning(block.details)}
									<ConversationWorkSession
										awaiting_compaction={awaiting_compaction &&
											block.session.run_id === active_run_id}
										background_agent_names={conversation_background_agent_names(
											block.details,
										)}
										duration_kind={block.duration_kind}
										engine_id={policy?.engine_id}
										has_details={visible_details.length > 0}
										has_live_reply={conversation_reply_is_live(block.details)}
										item={block.session}
										onretry={block.session.status === "failed" ? onretry : undefined}
										progress_phase={conversation_progress_phase(block.details)}
										reasoning_summary={block.session.run_id === active_run_id
											? live_reasoning_summary
											: undefined}
										steering_pending={steering_pending &&
											!session_settled &&
											active_run_id !== undefined &&
											block.session.run_id === active_run_id}
										superseded={block.superseded === true}
										transition={block.transition}
										waiting_for_activity={conversation_waiting_for_activity(block.details)}
									>
									{#snippet details(session_failed: boolean)}
										<!-- Stopping is the user's own act, not a failure the trace must explain. -->
										<ConversationTrace
											failed={session_failed}
											items={visible_details}
											work_active={!session_settled &&
												block.session.ended_at === undefined}
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

	{#if !inspecting_agent}
		<ThreadComposer
			{context_usage}
			{disabled}
			draft_key={snapshot.thread_id}
			{engine_locked}
			{onabort}
			onjumptolatest={JumpToLatest}
			{onnewthread}
			{onpolicychange}
			onsteeringchange={SetSteeringPending}
			onsubmit={onsubmit === undefined || active_run_status === "queued"
				? undefined
				: SubmitMessage}
			{onwithdraw}
			{policy}
			{run_active}
			show_jump_to_latest={!following && !anchor_scroll_active}
		/>
	{/if}
</main>

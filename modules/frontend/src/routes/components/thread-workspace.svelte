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
		conversation_reply_is_confirmed,
		conversation_reply_is_live,
		conversation_run_presentation_is_active,
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
		ConversationHasRemoteOlderTurns,
		ConversationOlderGroupCountForItem,
		MakeParticipantConversationRenderWindow,
		type ConversationRenderBlock,
		type ConversationViewState,
	} from "$lib/conversation/store";
	import {
		ConversationVisualSettlementDeadlineMillis,
		ConversationVisualSettlementDecision,
		ConversationVisualSettlementSampleMillis,
		type ConversationVisualSettlementMeasurement,
	} from "$lib/conversation/visual-settlement";
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
		onhydrateolder,
		onapproval,
		onnewthread,
		onpolicychange,
		onquestion,
		onretry,
		onusageinterruptionresolve,
		onimagevisibilitychange,
		onsubmit,
		onvisualsettled,
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
		/**
		 * Loads one older durable-history range beneath the loaded window,
		 * optionally down to a target turn. False means nothing further loaded.
		 */
		onhydrateolder?: (minimum_turn_ordinal?: number) => Effect.Effect<boolean>;
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
		onvisualsettled?: (
			measurement: ConversationVisualSettlementMeasurement,
		) => Effect.Effect<void>;
		/** Recalls one queued steer by the send's own command id, while Forge still holds it. */
		onwithdraw?: (command_id: string) => Effect.Effect<void, { readonly message: string }>;
		policy?: ThreadSessionPolicy;
		project_root_path?: string;
		run_active?: boolean;
		snapshot: ConversationSnapshot;
	} = $props();
	let visual_settlement_started_at = 0;
	if (untrack(() => onvisualsettled !== undefined)) {
		visual_settlement_started_at = yield* RunBrowserDom(() =>
			globalThis.performance.now(),
		).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					return 0;
				}),
			),
		);
	}
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
	/** Transcript authority can retain the live session while work authority crosses settlement. */
	const transcript_live_run_id = $derived(
		snapshot.items.findLast(
			(item) => item.type === "work_session" && !work_session_is_settled(item.status),
		)?.run_id,
	);
	const presentation_run_id = $derived(
		run_active && active_run_id !== undefined
			? active_run_id
			: (transcript_live_run_id ?? active_run_id),
	);
	/**
	 * Controls follow the work subscription immediately; transcript chrome does
	 * not. Keep the latter live until its own session settles, so an earlier work
	 * settlement cannot erase the summary before the final-message patch lands.
	 */
	const presentation_run_active = $derived(
		conversation_run_presentation_is_active(
			snapshot.items,
			presentation_run_id,
			run_active && presentation_run_id === active_run_id,
		),
	);
	/** Only a currently live owning turn may opt prose into word-by-word reveal. */
	const streaming_turn_ids = $derived(
		new Set(
			snapshot.turns
				.filter((turn) => turn.lifecycle === "active" || turn.lifecycle === "streaming")
				.map((turn) => turn.id),
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
			: conversation_live_reasoning_summary(
					render_blocks,
					presentation_run_id,
					presentation_run_active,
				),
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
	let steering_pending_source_reference = $state<string | undefined>();
	const SetSteeringPending = (pending: boolean, source_reference?: string) => {
		steering_pending_source_reference = pending ? source_reference : undefined;
	};
	const ReturnToRoot = Effect.gen(function* () {
		if (onreturntoroot !== undefined) yield* onreturntoroot;
	});
	const ReturnToRootOnEscape = (event: KeyboardEvent) =>
		Effect.gen(function* () {
			if (event.key === "Escape" && inspecting_agent) yield* ReturnToRoot;
		});

	let remote_history_exhausted = $state(false);
	const has_remote_older_turns = $derived(
		!remote_history_exhausted && ConversationHasRemoteOlderTurns(snapshot),
	);

	const ShowEarlierTurns = Effect.gen(function* () {
		if (loading_older_turns) return;
		loading_older_turns = true;
		yield* Effect.gen(function* () {
			/** Refill the hidden pool from durable history before revealing it. */
			if (
				hidden_render_group_count < ConversationTurnPageSize &&
				has_remote_older_turns &&
				onhydrateolder !== undefined
			) {
				const hydrated = yield* onhydrateolder();
				if (!hydrated) remote_history_exhausted = true;
			}
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

	let workspace_surface = $state<HTMLElement | null>(null);
	let viewport = $state<HTMLElement | null>(null);
	let transcript_content = $state<HTMLElement | null>(null);
	let end_space = $state<HTMLElement | null>(null);
	let end_space_height = $state(ConversationBaseEndSpacePixels);
	const initial_submission_reference = untrack(() => first_submission_reference);
	/** Seeded with a claimed first submission so a new thread's opening turn anchors like any other send. */
	let pending_user_message_reference = $state<string | undefined>(initial_submission_reference);
	let anchored_user_item_id = $state<string | undefined>();
	/**
	 * Whether new content should pull the viewport down with it. Derived from
	 * scroll position on every unowned scroll, so the reader is never in a mode they did
	 * not put themselves in — scrolling away turns it off, returning to the
	 * bottom turns it back on. A seeded first submission starts it off, exactly
	 * as an ordinary send switches it off at submission, so the tail cannot pull
	 * the reader while that turn's anchor is still on its way.
	 */
	let following = $state(initial_submission_reference === undefined);
	/**
	 * Set while the anchor's visual correction animates. The viewport itself is
	 * already at the authoritative destination, so its programmatic event must
	 * not re-arm following before the pixels catch up.
	 */
	let anchor_scroll_active = $state(false);
	let anchor_scroll_generation = 0;
	/**
	 * True while Artisan still owns the sent turn's reading position. A wheel or
	 * touch gesture gives that position back to the reader; layout corrections
	 * must never pull them back after that.
	 */
	let anchor_position_owned = false;
	let anchor_initial_layout_pending = false;
	let pending_anchor_owned = initial_submission_reference !== undefined;
	let anchor_scroll_releases_on_scroll_end = false;
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
		if (anchor_scroll_active || pending_anchor_owned) return;
		following = ConversationIsFollowing(
			element.scrollTop,
			element.scrollHeight,
			element.clientHeight,
		);
	};

	const release_anchor_scroll = (element: HTMLElement) => {
		const sync_following = anchor_scroll_releases_on_scroll_end;
		anchor_scroll_generation += 1;
		anchor_scroll_active = false;
		anchor_scroll_releases_on_scroll_end = false;
		if (sync_following) SyncFollowing(element);
		else if (anchor_position_owned) RequestAnchorLayoutUnsafe(false);
	};

	const ReleaseAnchorPosition = () => {
		pending_anchor_owned = false;
		pending_user_message_reference = undefined;
		anchor_position_owned = false;
		anchor_initial_layout_pending = false;
		if (anchor_scroll_active && viewport !== null) release_anchor_scroll(viewport);
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
	 * Moves the transcript visually after the viewport has already landed at the
	 * sent turn. Unlike a native smooth scroll, this cannot be abandoned at an
	 * arbitrary intermediate scrollTop when streaming content changes layout.
	 */
	const GlideAnchorCorrection = (content: HTMLElement, delta: number) => {
		if (delta === 0 || reduced_motion) return;
		content.style.transition = "none";
		content.style.transform = `translateY(${delta}px)`;
		void content.offsetHeight;
		content.style.transition = "transform var(--duration-fast) var(--ease-smooth-out)";
		content.style.transform = "translateY(0px)";
	};

	/**
	 * Guards programmatic movement, but never relies exclusively on `scrollend`:
	 * no-movement, visual-only glides, and older runtimes can omit it.
	 * The generation fence makes a newer scroll own the one-second fallback.
	 */
	const ArmAnchorScroll = (
		element: HTMLElement,
		next_following: boolean,
		release_on_scroll_end = true,
		fallback_millis = 1_000,
	) =>
		Effect.gen(function* () {
			const generation = (anchor_scroll_generation += 1);
			following = next_following;
			anchor_scroll_active = true;
			anchor_scroll_releases_on_scroll_end = release_on_scroll_end;
			/** Forked into the component scope: the fallback must outlive the statement rerun that armed it. */
			yield* Effect.gen(function* () {
				yield* Effect.sleep(fallback_millis);
				if (generation === anchor_scroll_generation) release_anchor_scroll(element);
			}).pipe(Effect.forkIn(anchor_scope));
		});

	/**
	 * The transcript's own map, down the right edge of the card.
	 *
	 * Position is read from where the turns actually sit rather than tracked as
	 * the reader moves: scrolling is the only thing that changes it, the marks
	 * are few, and a measurement taken on the spot cannot drift out of step with
	 * a transcript that grows and reflows underneath it.
	 */
	const turn_markers = $derived(ConversationTurnMarkers(snapshot));
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
			/** A marker below the loaded floor hydrates durable history first. */
			if (
				item === undefined &&
				onhydrateolder !== undefined &&
				marker.turn_ordinal !== undefined
			) {
				let attempts = 0;
				while (
					attempts < 32 &&
					conversation_view_state?.items_by_id.has(marker.id) !== true
				) {
					attempts += 1;
					const hydrated = yield* onhydrateolder(marker.turn_ordinal);
					if (!hydrated) break;
				}
			}
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
			anchor_position_owned &&
			!anchor_initial_layout_pending &&
			!following &&
			!anchor_scroll_active &&
			Math.abs(
				ConversationAlignedScrollTop(viewport_scroll_top, viewport_top, item_bounds.top) -
					viewport_scroll_top,
			) <= 32
		) {
			anchor_position_owned = false;
			following = true;
		}
		if (next_end_space_height !== end_space_height) {
			end_space_height = next_end_space_height;
			yield* Effect.promise(() => tick());
		}
		if (
			viewport === null ||
			anchored_user_item_id !== item_id ||
			!anchor_position_owned ||
			(!smooth && (anchor_initial_layout_pending || anchor_scroll_active))
		)
			return;

		const current_item = yield* FindConversationItem(item_id);
		if (current_item === undefined) return;
		if (smooth) {
			/**
			 * Own the scroll before assigning it. Programmatic scroll events are
			 * queued by Chromium, but an already-pending scrollend can otherwise
			 * release a newly armed move before its first painted frame.
			 */
			yield* ArmAnchorScroll(
				viewport,
				false,
				false,
				reduced_motion ? 0 : 350,
			);
		}
		yield* RunBrowserDom(() => {
			const previous_scroll_top = viewport.scrollTop;
			viewport.scrollTo({
				behavior: "auto",
				top: ConversationAlignedScrollTop(
					viewport.scrollTop,
					viewport.getBoundingClientRect().top,
					current_item.getBoundingClientRect().top,
				),
			});
			if (smooth && transcript_content !== null) {
				GlideAnchorCorrection(
					transcript_content,
					viewport.scrollTop - previous_scroll_top,
				);
			}
		});
		if (smooth && anchored_user_item_id === item_id) anchor_initial_layout_pending = false;
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
		pending_anchor_owned = true;
		/** Freeze the old tail before the accepted message can race its receipt. */
		following = false;
		const ClearPendingUserMessage = Effect.gen(function* () {
			pending_user_message_reference = undefined;
			pending_anchor_owned = false;
			if (viewport !== null) SyncFollowing(viewport);
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
					pending_user_message_reference =
						outcome.user_message_reference !== undefined && pending_anchor_owned
							? outcome.user_message_reference
							: undefined;
					if (outcome.user_message_reference === undefined && viewport !== null) {
						pending_anchor_owned = false;
						SyncFollowing(viewport);
					}
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
		pending_anchor_owned = false;
		anchored_user_item_id = undefined;
		anchor_position_owned = false;
		anchor_initial_layout_pending = false;
		end_space_height = ConversationBaseEndSpacePixels;
		yield* Effect.promise(() => tick());
		yield* ArmAnchorScroll(current_viewport, true);
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
				if (anchored_user_item_id === item_id) {
					pending_anchor_owned = false;
					pending_user_message_reference = undefined;
					return;
				}
				anchored_user_item_id = item_id;
				anchor_position_owned = true;
				anchor_initial_layout_pending = true;
				pending_anchor_owned = false;
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
	/**
	 * The bound elements are arguments, not reads hidden inside the Effect. SER
	 * derives a yielded program's reactive inputs from its call expression; when
	 * this took no arguments it ran once with both bindings null and never
	 * attached after mount. The opening bottom assignment then lost every race
	 * with Markdown, highlighting, images, and cards that grew afterwards.
	 */
	const SyncTranscriptSizeObserver = (
		content: HTMLElement | null,
		current_viewport: HTMLElement | null,
	) =>
		Effect.gen(function* () {
			const supports_resize_observer = yield* RunBrowserDom(
				() => "ResizeObserver" in globalThis,
			);
			if (content === null || current_viewport === null || !supports_resize_observer) {
				yield* transcript_size_observers.Release("transcript-size");
				return;
			}
			yield* transcript_size_observers.Replace("transcript-size", {
				content,
				current_viewport,
			});
		});
	yield* SyncTranscriptSizeObserver(transcript_content, viewport);

	type LayoutShiftEntry = PerformanceEntry & {
		readonly hadRecentInput: boolean;
		readonly sources?: ReadonlyArray<{ readonly node?: Node | null }>;
		readonly value: number;
	};
	type VisualSettlementTarget = {
		readonly content: HTMLElement;
		readonly current_viewport: HTMLElement;
		readonly onsettled: (
			measurement: ConversationVisualSettlementMeasurement,
		) => Effect.Effect<void>;
		readonly started_at: number;
		readonly surface: HTMLElement;
		readonly thread_id: string;
	};
	type VisualSettlementDomSample = {
		readonly content_height: number;
		readonly content_width: number;
		readonly fonts_loaded: boolean;
		readonly now: number;
		readonly pending_transforms: boolean;
		readonly positioned: boolean;
		readonly viewport_height: number;
		readonly viewport_width: number;
	};

	let visual_settlement_reported = false;
	const RoundVisualMeasurement = (value: number): number =>
		Math.round(value * 1_000) / 1_000;

	/**
	 * The snapshot is only data-ready. This observer measures the rendered
	 * transcript through Markdown/highlighting/math/Mermaid upgrades, font loads,
	 * and the opening scroll assignment. The route remains mounted behind its
	 * spinner during this one-shot pass, then reveals on a quiet paint boundary.
	 */
	const ObserveInitialVisualSettlement = (target: VisualSettlementTarget) =>
		Effect.gen(function* () {
			yield* Effect.scoped(
				Effect.gen(function* () {
					let layout_revision = 0;
					let layout_shift_score = 0;
					let mutation_count = 0;
					let resize_count = 0;

					const observers = yield* RunBrowserDom(() => {
						const resize_observer = new ResizeObserver(() => {
							resize_count += 1;
							layout_revision += 1;
						});
						resize_observer.observe(target.content);
						resize_observer.observe(target.current_viewport);

						const mutation_observer = new MutationObserver((records) => {
							mutation_count += records.length;
							layout_revision += 1;
						});
						mutation_observer.observe(target.surface, {
							attributes: true,
							characterData: true,
							childList: true,
							subtree: true,
						});

						let layout_shift_observer: PerformanceObserver | undefined;
						if (
							typeof globalThis.PerformanceObserver === "function" &&
							globalThis.PerformanceObserver.supportedEntryTypes.includes("layout-shift")
						) {
							layout_shift_observer = new PerformanceObserver((list) => {
								for (const candidate of list.getEntries()) {
									const entry = candidate as LayoutShiftEntry;
									if (entry.hadRecentInput || entry.startTime < target.started_at) continue;
									const belongs_to_workspace =
										entry.sources?.some(
											(source) =>
												source.node !== null &&
												source.node !== undefined &&
												target.surface.contains(source.node),
										) ?? false;
									if (belongs_to_workspace) layout_shift_score += entry.value;
								}
							});
							layout_shift_observer.observe({ buffered: true, type: "layout-shift" });
						}

						return { layout_shift_observer, mutation_observer, resize_observer };
					});
					yield* Effect.addFinalizer(() =>
						RunBrowserDom(() => {
							observers.layout_shift_observer?.disconnect();
							observers.mutation_observer.disconnect();
							observers.resize_observer.disconnect();
						}).pipe(Effect.ignore),
					);

					const ReadSample = () =>
						RunBrowserDom((): VisualSettlementDomSample => {
							const bounds = target.content.getBoundingClientRect();
							return {
								content_height: bounds.height,
								content_width: bounds.width,
								fonts_loaded: globalThis.document.fonts.status === "loaded",
								now: globalThis.performance.now(),
								pending_transforms:
									target.content.querySelector(
										'[data-thread-content-transforming="true"], [aria-busy="true"]',
									) !== null,
								positioned,
								viewport_height: target.current_viewport.clientHeight,
								viewport_width: target.current_viewport.clientWidth,
							};
						});

					let previous = yield* ReadSample();
					let observed_revision = layout_revision;
					let stable_sample_count = 0;
					let last_change_at = previous.now;
					let content_height_shift_px = 0;
					let largest_content_height_shift_px = 0;
					let reason: "stable" | "deadline" | undefined;

					while (reason === undefined) {
						yield* Effect.sleep(ConversationVisualSettlementSampleMillis);
						const current = yield* ReadSample();
						const height_shift = Math.abs(current.content_height - previous.content_height);
						const sample_changed =
							layout_revision !== observed_revision ||
							height_shift > 0.25 ||
							Math.abs(current.content_width - previous.content_width) > 0.25 ||
							current.viewport_height !== previous.viewport_height ||
							current.viewport_width !== previous.viewport_width ||
							current.pending_transforms !== previous.pending_transforms ||
							current.fonts_loaded !== previous.fonts_loaded ||
							current.positioned !== previous.positioned;

						if (height_shift > 0.25) {
							content_height_shift_px += height_shift;
							largest_content_height_shift_px = Math.max(
								largest_content_height_shift_px,
								height_shift,
							);
						}
						if (sample_changed) {
							last_change_at = current.now;
							stable_sample_count = 0;
						} else {
							stable_sample_count += 1;
						}

						observed_revision = layout_revision;
						previous = current;
						reason = ConversationVisualSettlementDecision({
							elapsed_ms: current.now - target.started_at,
							fonts_loaded: current.fonts_loaded,
							maximum_wait_ms: ConversationVisualSettlementDeadlineMillis(run_active),
							pending_transforms:
								current.pending_transforms || !current.positioned,
							quiet_ms: current.now - last_change_at,
							stable_sample_count,
						});
					}

					const measurement: ConversationVisualSettlementMeasurement = {
						content_height_shift_px: RoundVisualMeasurement(content_height_shift_px),
						duration_ms: RoundVisualMeasurement(previous.now - target.started_at),
						largest_content_height_shift_px: RoundVisualMeasurement(
							largest_content_height_shift_px,
						),
						layout_shift_score: RoundVisualMeasurement(layout_shift_score),
						mutation_count,
						pending_transforms_at_reveal: previous.pending_transforms,
						reason,
						resize_count,
					};
					yield* RunBrowserDom(() => {
						globalThis.performance.measure("artisan.thread.visual-settlement", {
							detail: { ...measurement, thread_id: target.thread_id },
							end: previous.now,
							start: target.started_at,
						});
					}).pipe(Effect.ignore);
					if (visual_settlement_reported) return;
					visual_settlement_reported = true;
					yield* target.onsettled(measurement);
				}),
			);
		}).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					if (visual_settlement_reported) return;
					visual_settlement_reported = true;
					yield* target.onsettled({
						content_height_shift_px: 0,
						duration_ms: 0,
						largest_content_height_shift_px: 0,
						layout_shift_score: 0,
						mutation_count: 0,
						pending_transforms_at_reveal: true,
						reason: "measurement_unavailable",
						resize_count: 0,
					});
				}),
			),
		);

	const visual_settlement_observers = yield* MakeScopedAttachmentRunner(
		ObserveInitialVisualSettlement,
	);
	const SyncVisualSettlementObserver = (
		surface: HTMLElement | null,
		content: HTMLElement | null,
		current_viewport: HTMLElement | null,
		view_ready: boolean,
		onsettled:
			| ((measurement: ConversationVisualSettlementMeasurement) => Effect.Effect<void>)
			| undefined,
	) =>
		Effect.gen(function* () {
			if (
				surface === null ||
				content === null ||
				current_viewport === null ||
				!view_ready ||
				onsettled === undefined
			) {
				yield* visual_settlement_observers.Release("initial-visual-settlement");
				return;
			}
			if (visual_settlement_reported) return;
			yield* visual_settlement_observers.Replace("initial-visual-settlement", {
				content,
				current_viewport,
				onsettled,
				started_at: visual_settlement_started_at,
				surface,
				thread_id: snapshot.thread_id,
			});
		});
	yield* SyncVisualSettlementObserver(
		workspace_surface,
		transcript_content,
		viewport,
		conversation_view_state !== undefined,
		onvisualsettled,
	);

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
						const on_scroll_end = () => {
							if (anchor_scroll_releases_on_scroll_end)
								release_anchor_scroll(current_viewport);
						};
						const on_user_scroll_intent = () => {
							if (anchor_scroll_active && !anchor_scroll_releases_on_scroll_end) {
								const content = transcript_content;
								if (content !== null) {
									content.style.transition = "none";
									content.style.transform = "translateY(0px)";
								}
							}
							ReleaseAnchorPosition();
						};
						current_viewport.addEventListener("scroll", on_scroll, { passive: true });
						current_viewport.addEventListener("scrollend", on_scroll_end, {
							passive: true,
						});
						current_viewport.addEventListener("touchstart", on_user_scroll_intent, {
							passive: true,
						});
						current_viewport.addEventListener("wheel", on_user_scroll_intent, {
							passive: true,
						});
						return { on_scroll, on_scroll_end, on_user_scroll_intent };
					}),
					(handlers) =>
						RunBrowserDom(() => {
							current_viewport.removeEventListener("scroll", handlers.on_scroll);
							current_viewport.removeEventListener("scrollend", handlers.on_scroll_end);
							current_viewport.removeEventListener(
								"touchstart",
								handlers.on_user_scroll_intent,
							);
							current_viewport.removeEventListener("wheel", handlers.on_user_scroll_intent);
						}),
				);
				yield* Effect.never;
			}),
	);
	const SyncFollowListeners = (current_viewport: HTMLElement | null) =>
		Effect.gen(function* () {
			if (current_viewport === null) {
				yield* follow_listeners.Release("follow");
				return;
			}
			yield* follow_listeners.Replace("follow", { current_viewport });
		});
	yield* SyncFollowListeners(viewport);
</script>

<svelte:window onkeydown={yield* ReturnToRootOnEscape(event)} />

<main
	bind:this={workspace_surface}
	class="relative h-full min-h-0 overflow-hidden"
	aria-label={inspecting_agent ? "Agent conversation" : "Thread workspace"}
>
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
					{#if hidden_render_group_count > 0 || has_remote_older_turns}
						<div class="flex justify-center pb-2">
							<Button
								variant="ghost"
								size="sm"
								disabled={loading_older_turns}
								onclick={yield* ShowEarlierTurns}
							>
								{hidden_render_group_count > 0
									? `Show earlier turns (${hidden_render_group_count})`
									: "Show earlier turns"}
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
									{@const block_run_active = run_active &&
										block.items.some((item) => item.run_id === active_run_id)}
									<ConversationTrace
										items={visible_trace_items}
										message_streaming={block_run_active &&
											streaming_turn_ids.has(block.turn_id)}
										work_active={block_run_active}
									/>
								{:else if block.type === "item"}
									<ConversationItem
										{image_sources}
										item={block.item}
										message_streaming={run_active &&
											block.item.run_id === active_run_id &&
											streaming_turn_ids.has(block.turn_id)}
										{onapproval}
										{onimagevisibilitychange}
										{onquestion}
										{onusageinterruptionresolve}
										steering_pending={steering_pending_source_reference !== undefined &&
											block.item.type === "user_message" &&
											block.item.source_refs.some(
												(source) =>
													source.reference === steering_pending_source_reference ||
													source.event_id === steering_pending_source_reference,
											)}
									/>
								{:else if block.type === "work_group"}
									{@const session_settled = work_session_is_settled(block.session.status)}
									{@const visible_details = strip_conversation_trace_reasoning(block.details)}
									{@const progress_items = block.progress_items ?? [block.session, ...block.details]}
									<ConversationWorkSession
										awaiting_compaction={awaiting_compaction &&
											block.session.run_id === active_run_id}
										background_agent_names={conversation_background_agent_names(
											progress_items,
										)}
										duration_kind={block.duration_kind}
										engine_id={policy?.engine_id}
										has_details={visible_details.length > 0}
										has_live_reply={block.progress_phase === "reply" &&
											conversation_reply_is_live(progress_items)}
										item={block.session}
										onretry={block.session.status === "failed" ? onretry : undefined}
										progress_phase={block.progress_phase}
										reply_confirmed={conversation_reply_is_confirmed(progress_items)}
										reasoning_summary={block.session.run_id === presentation_run_id
											? live_reasoning_summary
											: undefined}
										superseded={block.superseded === true}
										transition={block.transition}
										waiting_for_activity={conversation_waiting_for_activity(
											progress_items,
										)}
									>
									{#snippet details(session_failed: boolean)}
										<!-- Stopping is the user's own act, not a failure the trace must explain. -->
										<ConversationTrace
											failed={session_failed}
											items={visible_details}
											message_streaming={run_active &&
												block.session.run_id === active_run_id &&
												!session_settled &&
												block.session.ended_at === undefined &&
												streaming_turn_ids.has(block.turn_id)}
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

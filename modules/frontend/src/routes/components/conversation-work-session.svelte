<script lang="ts" effect>
	import type { ConversationItem } from "@artisan/protocol";
	import { artisan_error_codes } from "@artisan/catalog";
	import Refresh from "@tabler/icons-svelte/icons/refresh";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import { Effect } from "effect";
	import { untrack } from "svelte";
	import type { Snippet } from "svelte";
	import {
		active_work_label_for,
		thinking_word_for,
		work_session_settlement,
		type WorkSessionRunAuthority,
	} from "$lib/conversation/activity-status";
	import { EngineDisplayName } from "$lib/engine/presentation";
	import { work_session_disclosure } from "$lib/conversation/presentation";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import { Button } from "$lib/components/ui/button";
	import ConversationStatus from "./conversation-status.svelte";
	import ConversationErrorCard from "./conversation-error-card.svelte";

	let {
		engine_id,
		has_live_reply = false,
		has_details = false,
		item,
		details,
		duration_kind,
		onretry,
		run_authority = "settled",
		transition,
		waiting_for_activity = false,
	}: {
		/**
		 * Who the request is out to. Known only from the thread's policy — the
		 * work item itself is engine-neutral — so an unattributed session simply
		 * keeps its thinking word.
		 */
		engine_id?: string;
		/**
		 * True while the assistant's prose is streaming below with something on
		 * screen. A reply is its own status, so the session adds no line of its
		 * own while one arrives. Tools and reasoning are deliberately not replies:
		 * the word holds across the whole chain rather than blinking through it.
		 */
		has_live_reply?: boolean;
		/** Settled traces stay unmounted until disclosure, so the header needs this hint. */
		has_details?: boolean;
		item: Extract<ConversationItem, { type: "work_session" }>;
		/** Receives this session's authority-aware failure verdict for its nested trace. */
		details?: Snippet<[failed: boolean]>;
		duration_kind?: "thought" | "worked";
		onretry?: (
			run_id: string,
		) => Effect.Effect<void, { readonly message: string }>;
		/**
		 * The durable work item's verdict on this session's run. The authority on
		 * liveness is that item plus run identity, not this session's own
		 * lifecycle, which a killed run can leave unterminated. An unattributed
		 * session defaults to settled history rather than eternal work.
		 */
		run_authority?: WorkSessionRunAuthority;
		/** The engine handoff that started this run, shown at the header's far end. */
		transition?: Extract<ConversationItem, { type: "model_transition" }>;
		/** True between canonical activity start and terminal events for this turn. */
		waiting_for_activity?: boolean;
	} = $props();
	/** Live work and unsuccessful settlements open by default; the reader remains in control. */
	let open = $state(
		untrack(
			() =>
				item.ended_at === undefined ||
				item.status === "failed" ||
				item.status === "cancelled",
		),
	);
	let user_chose_disclosure = $state(false);
	let previous_status = untrack(() => item.status);
	let has_visible_details = $state(untrack(() => has_details));
	let has_live_status_detail = $state(false);
	let status_line_was_visible = $state(false);
	let status_line_has_appeared = $state(false);
	let thinking_visibility_generation = $state(0);
	/** The settled label's measured width — where the divider starts its growth. */
	let label_width = $state(0);
	type DetailObservation =
		| { readonly _tag: "Observe"; readonly element: HTMLDivElement; readonly refresh_key: string }
		| { readonly _tag: "Refresh"; readonly element: HTMLDivElement };

	const FormatDuration = (started_at: string, ended_at: string) => {
		const total_seconds = Math.max(
			0,
			Math.floor((Date.parse(ended_at) - Date.parse(started_at)) / 1_000),
		);
		const hours = Math.floor(total_seconds / 3_600);
		const minutes = Math.floor((total_seconds % 3_600) / 60);
		const seconds = total_seconds % 60;

		return [
			hours > 0 ? `${hours}h` : undefined,
			minutes > 0 || hours > 0 ? `${minutes}m` : undefined,
			`${seconds}s`,
		]
			.filter((part) => part !== undefined)
			.join(" ");
	};

	/**
	 * Captured at mount on purpose: a session that was already settled when it
	 * mounted is history and renders its header still. Only a session observed
	 * working here earns the settle entrance when its header replaces the word.
	 */
	const mounted_working = untrack(() => item.ended_at === undefined);

	/** One word per mounted quiet-status epoch, never a live carousel. */
	const thinking_word = $derived(thinking_word_for(item.id, thinking_visibility_generation));
	/**
	 * `responded_at` is set by the provider turn-start observation. Visible work
	 * remains a compatibility fallback for snapshots projected before that field
	 * existed, and also proves a response if a provider omits turn-start.
	 */
	const provider_responded = $derived(item.responded_at !== undefined || has_visible_details);
	/**
	 * A run can die without ever emitting its terminal lifecycle event — a Forge
	 * restart takes the engine process with it — which leaves this item with no
	 * `ended_at` and the transcript claiming to think forever. The durable work
	 * item is the authority on whether anything is still running: while it holds
	 * the run pending or active the session keeps waiting — the send gap must
	 * read as waiting, never "Thought for 0s" — and once it holds nothing live
	 * the session settles on when it last changed. A run that died before its
	 * first byte settles as the failure it is rather than waiting forever.
	 */
	const settlement = $derived(
		work_session_settlement({
			ended_at: item.ended_at,
			provider_responded,
			run_authority,
			updated_at: item.updated_at,
		}),
	);
	const ended_at = $derived(settlement?.ended_at);
	const is_failed = $derived(item.status === "failed" || settlement?.presumed_failed === true);
	const failure = $derived(
		item.failure ?? {
			code: artisan_error_codes.run_failed,
			detail: "No detailed reason was recorded for this failed run.",
		},
	);
	const retry_available = $derived(is_failed && item.run_id !== undefined && onretry !== undefined);
	let retrying = $state(false);
	let retry_failed = $state(false);
	const Retry = Effect.gen(function* () {
		const retry = onretry;
		const run_id = item.run_id;
		if (retry === undefined || run_id === undefined || retrying) return;
		retrying = true;
		retry_failed = false;
		yield* retry(run_id).pipe(
			Effect.matchEffect({
				onFailure: () =>
					Effect.gen(function* () {
						retrying = false;
						retry_failed = true;
					}),
				onSuccess: () =>
					Effect.gen(function* () {
						retrying = false;
					}),
			}),
		);
	});
	/** The user's own act reads as a stop, not as the run being cancelled on it. */
	const is_stopped = $derived(item.status === "cancelled");
	const label = $derived(
		ended_at === undefined
			? thinking_word
			: is_failed
				? `Failed after ${FormatDuration(item.started_at, ended_at)}`
				: is_stopped
					? `Stopped after ${FormatDuration(item.started_at, ended_at)}`
					: `${duration_kind === "worked" ? "Worked" : "Thought"} for ${FormatDuration(item.started_at, ended_at)}`,
	);
	const is_working = $derived(ended_at === undefined);
	/** A handoff run is answered by the engine it handed off to, not the one it left. */
	const responding_engine = $derived(transition?.target_engine_id ?? engine_id);
	/** Unattributed work keeps its verb rather than reading "Waiting for Other". */
	const responding_name = $derived(
		responding_engine === undefined ? undefined : EngineDisplayName(responding_engine),
	);
	const renders_status_line = $derived(
		is_working && !has_live_reply && !has_live_status_detail,
	);
	const status_label = $derived(
		is_working
			? active_work_label_for({
					engine_name: responding_name,
					provider_responded,
					seed: item.id,
					thinking_visibility_generation,
					waiting_for_activity,
				})
			: label,
	);
	const disclosure = $derived(
		work_session_disclosure({
			details_defined: details !== undefined,
			has_visible_details,
			open,
			working: is_working,
		}),
	);

	/**
	 * A failure observed live opens once. After the user touches disclosure,
	 * later projection refreshes must not fight their explicit choice.
	 */
	const ReconcileStatus = (status: typeof item.status) =>
		Effect.gen(function* () {
			const became_unsuccessful =
				previous_status === "running" &&
				(status === "failed" || status === "cancelled");
			if (became_unsuccessful && !user_chose_disclosure) open = true;
			previous_status = status;
		});
	yield* ReconcileStatus(item.status);

	/**
	 * A quiet-status line earns a new word only after it was actually removed
	 * from the render tree for live detail. This reactive Effect is rerun by SER
	 * when the derived render condition changes; it never schedules a timer.
	 */
	const ReconcileThinkingVisibility = (status_line_visible: boolean) =>
		Effect.gen(function* () {
			if (status_line_visible && !status_line_was_visible) {
				if (status_line_has_appeared) thinking_visibility_generation += 1;
				status_line_has_appeared = true;
			}
			status_line_was_visible = status_line_visible;
		});
	yield* ReconcileThinkingVisibility(renders_status_line);

	/**
	 * A settled trace can receive its last detail after the session itself. While
	 * closed its DOM is intentionally absent, so no observer exists to discover
	 * that arrival; reconcile the cheap renderer hint instead. An open panel
	 * remains observer-owned, preserving the user's disclosure choice.
	 */
	const ReconcileClosedDetails = (
		details_available: boolean,
		disclosure_open: boolean,
		working: boolean,
	) =>
		Effect.gen(function* () {
			if (!working && !disclosure_open) has_visible_details = details_available;
		});
	yield* ReconcileClosedDetails(has_details, open, is_working);

	const RefreshDetails = (element: HTMLDivElement) =>
		Effect.gen(function* () {
			const details = yield* RunBrowserDom(() => ({
				has_live_status_detail: element.querySelector('[data-live-work-detail="true"]') !== null,
				has_visible_details: element.childElementCount > 0,
			}));
			has_visible_details = details.has_visible_details;
			has_live_status_detail = details.has_live_status_detail;
		});

	const ObserveDetails = (element: HTMLDivElement, refresh_key: string) =>
		Effect.gen(function* () {
			return yield* Effect.acquireRelease(
			Effect.gen(function* () {
				const observer = yield* RunBrowserDom(() => {
					const observer = new MutationObserver(() => detail_runner.ReplaceUnsafe(refresh_key, { _tag: "Refresh", element }));
					observer.observe(element, { childList: true, subtree: true });
					return observer;
				});
				detail_runner.ReplaceUnsafe(refresh_key, { _tag: "Refresh", element });
				return observer;
			}),
			(observer) =>
				Effect.gen(function* () {
					yield* RunBrowserDom(() => observer.disconnect());
					has_visible_details = has_details;
					has_live_status_detail = false;
				}),
			);
		});

	const RunDetailObservation = (observation: DetailObservation) =>
		Effect.gen(function* () {
			if (observation._tag === "Refresh") {
				yield* RefreshDetails(observation.element);
				return;
			}
			yield* ObserveDetails(observation.element, observation.refresh_key);
			yield* Effect.never;
		});

	const detail_runner = yield* MakeScopedAttachmentRunner(RunDetailObservation);

	/** Snippets are opaque; observe their rendered trace rather than treating their presence as content. */
	const observe_details = (element: HTMLDivElement) => {
		return detail_runner.Attachment({
			_tag: "Observe",
			element,
			refresh_key: `details-refresh:${crypto.randomUUID()}`,
		});
	};

	const ToggleDisclosure = () =>
		Effect.gen(function* () {
			user_chose_disclosure = true;
			open = !open;
		});
</script>

<section
	class="t-acc w-full text-base text-muted-foreground"
	data-open={disclosure.data_open}
	data-state={disclosure.data_state}
	data-has-header={disclosure.can_collapse ? "true" : undefined}
	aria-label={`${is_working ? status_label : label}: ${item.title}`}
>
	<!--
		The handoff has no header to sit in while the session is working, so it
		holds its own line until the settled header adopts it at the far end.
	-->
	{#if is_working && transition !== undefined}
		<div class="pb-2">
			<ConversationStatus item={transition} size="base" />
		</div>
	{/if}

	{#if disclosure.can_collapse}
		<div
			class={`t-settle-underline relative flex w-full items-center justify-between gap-3 pb-2 ${mounted_working ? "t-status-settle" : ""}`}
			style:--settle-underline-from={`${label_width}px`}
		>
			<button
				class="t-acc-head flex min-w-0 cursor-pointer items-center gap-1 text-left"
				type="button"
				aria-expanded={open}
				onclick={yield* ToggleDisclosure()}
			>
				<span
					class={`inline-block ${is_failed ? "text-destructive" : ""}`}
					bind:clientWidth={label_width}
				>
					{label}
				</span>
				<!-- The chevron belongs to the label, so it carries the label's tone. -->
				<ChevronRight
					class={`size-4 transition-transform duration-250 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none ${open ? "rotate-90" : ""} ${is_failed ? "text-destructive" : ""}`}
					aria-hidden="true"
				/>
			</button>
			{#if retry_available || transition !== undefined}
				<div class="flex shrink-0 items-center gap-2">
					{#if retry_available}
						<Button
							variant="ghost"
							size="sm"
							class="h-7 gap-1.5 px-2 text-muted-foreground hover:text-foreground"
							disabled={retrying}
							onclick={yield* Retry}
						>
							<Refresh
								class={`size-3.5 ${retrying ? "animate-spin motion-reduce:animate-none" : ""}`}
								aria-hidden="true"
							/>
							{retrying ? "Retrying…" : retry_failed ? "Retry again" : "Retry"}
						</Button>
					{/if}
					{#if transition !== undefined}
						<ConversationStatus item={transition} size="base" />
					{/if}
				</div>
			{/if}
		</div>
	{:else if !is_working}
		<div
			class={`t-settle-underline relative flex w-full items-center justify-between gap-3 pb-2 ${mounted_working ? "t-status-settle" : ""}`}
			style:--settle-underline-from={`${label_width}px`}
		>
			<span
				class={`inline-block ${is_failed ? "text-destructive" : ""}`}
				bind:clientWidth={label_width}
			>
				{label}
			</span>
			{#if retry_available || transition !== undefined}
				<div class="flex shrink-0 items-center gap-2">
					{#if retry_available}
						<Button
							variant="ghost"
							size="sm"
							class="h-7 gap-1.5 px-2 text-muted-foreground hover:text-foreground"
							disabled={retrying}
							onclick={yield* Retry}
						>
							<Refresh
								class={`size-3.5 ${retrying ? "animate-spin motion-reduce:animate-none" : ""}`}
								aria-hidden="true"
							/>
							{retrying ? "Retrying…" : retry_failed ? "Retry again" : "Retry"}
						</Button>
					{/if}
					{#if transition !== undefined}
						<ConversationStatus item={transition} size="base" />
					{/if}
				</div>
			{/if}
		</div>
	{/if}

	{#if is_failed}
		<!-- This must survive a closed trace: it is the run's explanation, not debug detail. -->
		<ConversationErrorCard error={failure} />
	{/if}

	{#if disclosure.details_mounted && details !== undefined}
		<div class="t-acc-panel" hidden={disclosure.details_hidden}>
			<div class="t-acc-panel-inner" use:observe_details>
				{@render details(is_failed)}
			</div>
		</div>
	{/if}

	<!--
		The status line lives at the end of the flow, never pinned above it: it is
		the latest thing happening, and it yields once the reply itself becomes the
		latest thing instead. Tools and reasoning running above it do not displace
		it — the word is what carries the reader across the whole chain.
	-->
	{#if renders_status_line}
		<div
			class={`t-status-enter flex w-fit items-center pb-0.5 ${has_visible_details ? "pt-5" : "pt-2"}`}
			role="status"
			aria-label={status_label}
		>
			<ShimmerText class="text-base text-muted-foreground" aria-hidden="true">
				{status_label}
			</ShimmerText>
		</div>
	{/if}
</section>

<style>
	/**
	 * The text-swap entrance in CSS alone: transition directives stall this
	 * tree under the experimental async renderer, so entrances play as mount
	 * animations and exits stay instant.
	 */
	.t-status-enter {
		animation: status-swap-enter var(--text-swap-dur) var(--ease-in-out) both;
	}

	/** The settled header holds one swap beat, reading as the word's replacement. */
	.t-status-settle {
		animation: status-swap-enter var(--text-swap-dur) var(--ease-in-out) var(--text-swap-dur)
			backwards;
	}

	@keyframes status-swap-enter {
		from {
			opacity: 0;
			transform: translateY(var(--text-swap-translate-y));
			filter: blur(var(--text-swap-blur));
		}
	}

	/**
	 * The divider is a pseudo-element rather than a border so its extent can
	 * animate. On settle it starts under the label's measured width and grows
	 * to the edge once the header's own entrance has landed.
	 */
	.t-settle-underline::after {
		content: "";
		position: absolute;
		bottom: 0;
		left: 0;
		height: 1px;
		width: 100%;
		background: var(--border);
	}

	.t-status-settle.t-settle-underline::after {
		animation: settle-underline-grow var(--duration-fast) var(--ease-smooth-out)
			calc(var(--text-swap-dur) * 2) both;
	}

	@keyframes settle-underline-grow {
		from {
			width: var(--settle-underline-from, 0px);
		}
	}

	.t-acc-panel {
		display: grid;
		grid-template-rows: 0fr;
		transition: grid-template-rows 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.t-acc[data-open="true"] .t-acc-panel {
		grid-template-rows: 1fr;
	}

	.t-acc-panel-inner {
		overflow: hidden;
		opacity: 0;
		filter: blur(2px);
		transition:
			opacity 250ms cubic-bezier(0.22, 1, 0.36, 1),
			filter 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.t-acc[data-has-header="true"][data-open="true"] .t-acc-panel-inner {
		padding-top: 0.5rem;
	}

	.t-acc[data-open="true"] .t-acc-panel-inner {
		opacity: 1;
		filter: blur(0);
	}

	@media (prefers-reduced-motion: reduce) {
		.t-status-enter,
		.t-status-settle,
		.t-status-settle.t-settle-underline::after {
			animation: none !important;
		}

		.t-acc-panel,
		.t-acc-panel-inner {
			transition: none !important;
		}
	}
</style>

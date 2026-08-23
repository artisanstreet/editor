<script lang="ts" effect>
	import type { ConversationItem } from "@artisan/protocol";
	import { artisan_error_codes } from "@artisan/catalog";
	import Refresh from "@tabler/icons-svelte/icons/refresh";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import { Clock, Effect } from "effect";
	import { untrack } from "svelte";
	import type { Snippet } from "svelte";
	import type { ConversationProgressPhase } from "$lib/conversation/activity-status";
	import {
		active_work_label_for,
		thinking_word_for,
		work_session_settlement,
	} from "$lib/conversation/activity-status";
	import InlineCodeText from "$lib/components/inline-code-text.svelte";
	import { EngineDisplayName } from "$lib/engine/presentation";
	import { FormatDuration } from "$lib/conversation/duration";
	import {
		work_session_disclosure,
		work_session_initially_open,
	} from "$lib/conversation/presentation";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import { Button } from "$lib/components/ui/button";
	import ConversationStatus from "./conversation-status.svelte";
	import ConversationErrorCard from "./conversation-error-card.svelte";

	let {
		awaiting_compaction = false,
		background_agent_names = [],
		engine_id,
		has_live_reply = false,
		has_details = false,
		item,
		details,
		duration_kind,
		onretry,
		reasoning_summary,
		superseded = false,
		transition,
		waiting_for_activity = false,
		progress_phase = "none",
		reply_confirmed = false,
	}: {
		/**
		 * True when the thread's own context reading says the engine must compact
		 * before it can answer. The engine publishes no frame while it does, so
		 * this is what keeps a multi-minute compaction from reading as a stall.
		 */
		awaiting_compaction?: boolean;
		/**
		 * Delegated workers still running behind the model's latest words. They are
		 * what holds the turn open once the reply has landed, so the quiet-status
		 * line names the wait on them instead of claiming private thought.
		 */
		background_agent_names?: ReadonlyArray<string>;
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
		 * the line holds across the whole chain rather than blinking through it.
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
		 * What this run's model last said it is thinking, for models that publish
		 * summaries. The status line is the one thinking line a turn has, so the
		 * summary belongs in it rather than beside it: a second shimmering line
		 * saying the same thing in fewer words is the presentation this replaces.
		 * Absent until the model has summarized anything, and always absent for
		 * models that stream raw thought instead.
		 */
		reasoning_summary?: string;
		/**
		 * True once later blocks of the same turn render below this session, which
		 * is what steering does to the work that follows it. The turn's one status
		 * line lives at the end of the flow, so a superseded session stops
		 * narrating: leaving it on pinned a live "Calculating" above the user's
		 * steering message while the work it described continued underneath.
		 */
		superseded?: boolean;
		/** The engine handoff that started this run, shown at the header's far end. */
		transition?: Extract<ConversationItem, { type: "model_transition" }>;
		/** True while the visible tool chain already communicates this turn's progress. */
		waiting_for_activity?: boolean;
		/** The newest visible reply-or-work phase in this session's durable order. */
		progress_phase?: ConversationProgressPhase;
		/**
		 * True once the newest prose finished without turning out to be tool
		 * narration. Only a confirmed reply may fold the trace into its summary.
		 */
		reply_confirmed?: boolean;
	} = $props();
	/**
	 * Settled work history arrives collapsed even when it has details or failed.
	 * Live work starts open, and a failure observed while mounted can still open
	 * once; an explicit close remains authoritative afterwards.
	 */
	let open = $state(
		untrack(
			() => work_session_initially_open({
				has_details,
				unsuccessful:
					item.status === "failed" ||
					item.status === "cancelled" ||
					item.status === "interrupted",
				working: item.ended_at === undefined,
			}),
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
	 * The session settles on its own projected status, which arrives in sequence
	 * with the transcript it belongs to. Nothing here consults a second source,
	 * so this header cannot outlive the run it describes.
	 */
	const settlement = $derived(
		work_session_settlement({
			ended_at: item.ended_at,
			status: item.status,
			updated_at: item.updated_at,
		}),
	);
	const ended_at = $derived(settlement?.ended_at);
	const is_failed = $derived(item.status === "failed");
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
	/**
	 * The user's own act reads as a stop, not as the run being cancelled on it —
	 * and so does a host restart. Neither is an error, so neither earns the
	 * failed tone: red is reserved for work that actually went wrong, which is
	 * the only way it keeps meaning anything.
	 */
	const is_stopped = $derived(item.status === "cancelled" || item.status === "interrupted");
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
	/**
	 * The header appears at the provider's first byte rather than at settlement,
	 * so the duration has to count while the run is still open.
	 *
	 * The loop's condition is the whole point: a transcript is almost entirely
	 * settled sessions, and forking an unconditional second-loop into every one
	 * of them had every session invalidating derived state once a second at
	 * once — which is what left the shell a render behind until a reload.
	 */
	let now_ms = $state(Date.now());
	/**
	 * This program is deliberately bound before the directly yielded site. SER
	 * reruns a directly yielded program when it observes reactive reads there;
	 * allowing the ticking `now_ms` write into that site reforked another scoped
	 * sleep loop on every second. The bound program is mounted once, and it
	 * reads the current settlement only from its owned fiber.
	 */
	const KeepElapsedLabelCurrent = Effect.gen(function* () {
		while (ended_at === undefined) {
			yield* Effect.sleep("1 second");
			now_ms = yield* Clock.currentTimeMillis;
		}
	});
	/** Settled history has no clock fiber; a live mount owns exactly one scoped ticker. */
	const clock_starts_live = untrack(() => ended_at === undefined);
	if (clock_starts_live) yield* KeepElapsedLabelCurrent.pipe(Effect.forkScoped);
	/**
	 * The header belongs to the turn from the moment the turn exists — the send,
	 * not the first token. Waiting for a provider to answer is part of what the
	 * turn cost, and a clock that only starts once the answer arrives under-reports
	 * exactly the wait the reader is sitting through. The status line below it
	 * still narrates ("Waiting for Claude"); this only counts.
	 *
	 * It is also where the model transition lands, which is why it is
	 * unconditional: a handover that arrived before the header did had nowhere to
	 * sit but the transcript, and read as a message rather than as a property of
	 * the turn.
	 */
	const elapsed_label = $derived(
		is_working
			? `${duration_kind === "worked" ? "Working" : "Thinking"} for ${FormatDuration(item.started_at, new Date(now_ms).toISOString())}`
			: label,
	);
	/** A handoff run is answered by the engine it handed off to, not the one it left. */
	const responding_engine = $derived(transition?.target_engine_id ?? engine_id);
	/** Unattributed work keeps its verb rather than reading "Waiting for Other". */
	const responding_name = $derived(
		responding_engine === undefined ? undefined : EngineDisplayName(responding_engine),
	);
	const renders_status_line = $derived(
		is_working &&
			!superseded &&
			!has_live_reply &&
			!has_live_status_detail &&
			!waiting_for_activity,
	);
	const status_label = $derived(
		is_working
			? active_work_label_for({
					awaiting_compaction,
					background_agent_names,
					engine_name: responding_name,
					provider_responded,
					reasoning_summary,
					seed: item.id,
					thinking_visibility_generation,
					waiting_for_activity,
				})
			: label,
	);
	/**
	 * A sentence needs a faster, undelayed sweep than a single word does. The
	 * default cadence was tuned for the verb; over a summary line the same band
	 * reads as a slow crawl.
	 */
	const says_summary = $derived(status_label === reasoning_summary);
	/**
	 * Disclosure belongs to the header, not to settlement: showing the header
	 * during a run without its control left the trace forced open with no way to
	 * shut it. Live work still arrives open — `open` is seeded true while the
	 * run is going — but the reader can close it.
	 */
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
				(status === "failed" || status === "cancelled" || status === "interrupted");
			if (became_unsuccessful && !user_chose_disclosure) open = true;
			previous_status = status;
		});
	yield* ReconcileStatus(item.status);

	let previous_progress_phase = untrack(() => progress_phase);
	let previous_working = untrack(() => item.ended_at === undefined);
	let previous_reply_confirmed = untrack(() => reply_confirmed || item.ended_at !== undefined);
	/**
	 * The reply retires the history it came from. Every unfinished session still
	 * starts open, including one mounted after prose has already begun; only a
	 * confirmed reply phase or successful settlement may fold it. Streaming
	 * prose is not confirmation — the provider has not yet said whether tool
	 * calls follow in the same message, and folding on it flashed the settled
	 * state in the middle of every working Claude turn. If activity or
	 * reasoning moves past prose, the chain is the progress again and opens. The
	 * durable cross-item order also catches completed prose batched with settlement
	 * and engines that leave an earlier text item streaming. Work wins whenever it
	 * is the newest visible phase.
	 * A reader's explicit toggle ends the automation for the session's whole life,
	 * and a failure still forces the trace open through the status reconcile above.
	 */
	const ReconcileReplyDisclosure = (
		phase: ConversationProgressPhase,
		working: boolean,
		status: typeof item.status,
		confirmed_reply: boolean,
	) =>
		Effect.gen(function* () {
			const phase_changed = phase !== previous_progress_phase;
			const settled = previous_working && !working;
			/** Settlement confirms by itself: nothing further can follow it. */
			const confirmed = confirmed_reply || !working;
			const confirmed_changed = confirmed !== previous_reply_confirmed;
			previous_progress_phase = phase;
			previous_working = working;
			previous_reply_confirmed = confirmed;
			if (user_chose_disclosure) return;
			if (status === "failed" || status === "cancelled" || status === "interrupted") return;
			if (phase === "work" && working) open = true;
			else if (
				phase === "reply" &&
				confirmed &&
				(phase_changed || settled || confirmed_changed)
			)
				open = false;
		});
	yield* ReconcileReplyDisclosure(progress_phase, is_working, item.status, reply_confirmed);

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
	class="t-acc group/session-acc w-full text-base text-muted-foreground"
	data-open={disclosure.data_open}
	data-state={disclosure.data_state}
	data-has-header={disclosure.can_collapse ? "true" : undefined}
	aria-label={`${is_working ? status_label : label}: ${item.title}`}
>
	<!--
		One header element for the session's whole life. It carries both the
		entrance and the divider's growth, so it must never be replaced: splitting
		the collapsible and plain headers into sibling branches remounted the whole
		thing the instant the first detail arrived, replaying the label's entrance
		and re-growing the bar under it — a flash on every turn's first message.
		Only the disclosure control appears, which is the one thing that genuinely
		became available.
	-->
	<div
		data-settled={mounted_working ? "true" : undefined}
		class={`t-settle-underline relative flex w-full items-center justify-between gap-3 pb-2 ${mounted_working ? "animate-[status-swap-enter_var(--text-swap-dur)_var(--ease-in-out)_var(--text-swap-dur)_backwards]" : ""}`}
		style:--settle-underline-from={`${label_width}px`}
	>
		{#if disclosure.can_collapse}
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
					{elapsed_label}
				</span>
				<!-- The chevron belongs to the label, so it carries the label's tone. -->
				<ChevronRight
					class={`size-4 transition-transform duration-250 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none ${open ? "rotate-90" : ""} ${is_failed ? "text-destructive" : ""}`}
					aria-hidden="true"
				/>
			</button>
		{:else}
			<span
				class={`inline-block ${is_failed ? "text-destructive" : ""}`}
				bind:clientWidth={label_width}
			>
				{elapsed_label}
			</span>
		{/if}
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

	{#if is_failed}
		<!-- This must survive a closed trace: it is the run's explanation, not debug detail. -->
		<ConversationErrorCard error={failure} />
	{/if}

	{#if disclosure.details_mounted && details !== undefined}
		<div class="t-acc-panel" hidden={disclosure.details_hidden}>
			<div
				class="t-acc-panel-inner group-data-[has-header=true]/session-acc:group-data-[open=true]/session-acc:pt-2"
				use:observe_details
			>
				{@render details(is_failed)}
			</div>
		</div>
	{/if}

	<!--
		The turn's one thinking line, at the end of the flow and never pinned above
		it: it is the latest thing happening, and it yields once the reply itself
		becomes the latest thing instead. A live tool chain already carries progress
		through its shimmer, so the row stays absent until that chain ends.

		It says the model's own latest summary when there is one and a thinking verb
		otherwise — one row for every state either way. A thought that outruns the
		row is a status, not content: it truncates into the trace's own
		clipped-command fade instead of opening downward, and thoughts the line no
		longer says stay retired with their phase. The element itself never changes
		across a thought swap: the shimmer's sweep restarts from nothing whenever
		its subtree is replaced.

		The trace and this line both use the header's half-rem spacing, keeping the
		divider equally inset from the content on either side.
	-->
	{#if renders_status_line}
		<div
			class="animate-[status-swap-enter_var(--text-swap-dur)_var(--ease-in-out)_both] my-2 flex max-w-(--prose-body-width) items-center"
			role="status"
			aria-label={status_label}
		>
			<!--
				The fade's mask lives on its own clipping span rather than on the
				shimmer: both utilities spell `animation`, and sharing one element
				would have whichever loses the cascade silently stand still.
			-->
			<span class="trace-command-label min-w-0 truncate" aria-hidden="true">
				<ShimmerText
					class="text-base leading-7 text-muted-foreground"
					delay={says_summary ? 0 : 1.5}
					duration={says_summary ? 2 : 3}
				>
					<!-- The model's own words, so its backticks read as the code face, not as marks. -->
					<InlineCodeText text={status_label} />
				</ShimmerText>
			</span>
		</div>
	{/if}
</section>

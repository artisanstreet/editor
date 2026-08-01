<script lang="ts" effect>
	import type { ConversationItem } from "@artisan/protocol";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import { Effect } from "effect";
	import { untrack } from "svelte";
	import type { Snippet } from "svelte";
	import { thinking_word_for } from "$lib/conversation/activity-status";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import ConversationStatus from "./conversation-status.sv";

	let {
		has_live_detail = false,
		item,
		details,
		duration_kind,
		transition,
	}: {
		/**
		 * True while some detail item is visibly working — a running command or
		 * streaming text. The latest live item is then the status, so the
		 * session adds no line of its own; the thinking word covers only the
		 * genuinely quiet stretches.
		 */
		has_live_detail?: boolean;
		item: Extract<ConversationItem, { type: "work_session" }>;
		details?: Snippet;
		duration_kind?: "thought" | "worked";
		/** The engine handoff that started this run, shown at the header's far end. */
		transition?: Extract<ConversationItem, { type: "model_transition" }>;
	} = $props();
	/** Failed work opens by default: its explanation must not hide behind a click. */
	let open = $state(untrack(() => item.status === "failed" || item.status === "cancelled"));
	let user_chose_disclosure = $state(false);
	let previous_status = untrack(() => item.status);
	let has_visible_details = $state(false);
	let has_live_status_detail = $state(false);
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

	const is_failed = $derived(item.status === "failed");
	const is_cancelled = $derived(item.status === "cancelled");
	/** One word for this session's whole life, chosen from its own identity. */
	const thinking_word = $derived(thinking_word_for(item.id));
	const label = $derived(
		item.ended_at === undefined
			? thinking_word
			: is_failed
				? `Failed after ${FormatDuration(item.started_at, item.ended_at)}`
				: is_cancelled
					? `Cancelled after ${FormatDuration(item.started_at, item.ended_at)}`
					: `${duration_kind === "worked" ? "Worked" : "Thought"} for ${FormatDuration(item.started_at, item.ended_at)}`,
	);
	const is_working = $derived(item.ended_at === undefined);
	const can_collapse = $derived(!is_working && has_visible_details);

	/**
	 * A failure observed live opens once. After the user touches disclosure,
	 * later projection refreshes must not fight their explicit choice.
	 */
	const ReconcileStatus = (status: typeof item.status) =>
		Effect.gen(function* () {
		const became_unsuccessful =
			previous_status === "running" && (status === "failed" || status === "cancelled");
		if (became_unsuccessful && !user_chose_disclosure) open = true;
		previous_status = status;
		});
	yield* ReconcileStatus(item.status);

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
					has_visible_details = false;
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
	data-open={is_working || can_collapse ? is_working || open : undefined}
	data-state={is_working || can_collapse ? (is_working || open ? "open" : "closed") : undefined}
	data-has-header={can_collapse ? "true" : undefined}
	aria-label={`${label}: ${item.title}`}
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

	{#if can_collapse}
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
				<ChevronRight
					class={`size-4 transition-transform duration-250 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none ${open ? "rotate-90" : ""}`}
					aria-hidden="true"
				/>
			</button>
			{#if transition !== undefined}
				<ConversationStatus item={transition} size="base" />
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
			{#if transition !== undefined}
				<ConversationStatus item={transition} size="base" />
			{/if}
		</div>
	{/if}

	{#if details !== undefined}
		<div class="t-acc-panel" hidden={!is_working && !has_visible_details}>
			<div class="t-acc-panel-inner" use:observe_details>
				{@render details()}
			</div>
		</div>
	{/if}

	<!--
		The status line lives at the end of the flow, never pinned above it: it is
		the latest thing happening, and it yields the moment a live detail — a
		running command, streaming text — becomes the latest thing instead.
	-->
	{#if is_working && !has_live_detail && !has_live_status_detail}
		<div
			class={`t-status-enter flex w-fit items-center pb-0.5 ${has_visible_details ? "pt-5" : "pt-0.5"}`}
			role="status"
			aria-label="Artisan is working"
		>
			<ShimmerText class="text-base text-muted-foreground" aria-hidden="true">
				{label}
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
		padding-top: 1rem;
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

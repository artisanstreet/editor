<script lang="ts" effect>
	import { untrack } from "svelte";
	import type { ThreadOpenSnapshot } from "@artisan/protocol";
	import { Effect } from "effect";
	import { FadeArc } from "$lib/components/ui/fade-arc";
	import type { ConversationVisualSettlementMeasurement } from "$lib/conversation/visual-settlement";
	import { DraftThreadController } from "$lib/root/draft-thread";
	import { ThreadOpenController } from "$lib/thread-interaction/thread-open-controller";
	import ThreadRoute from "./thread-route.svelte";

	let {
		thread_id: route_thread_id,
	}: {
		readonly thread_id: string;
	} = $props();
	const route_id = untrack(() => route_thread_id);
	const draft_thread = yield* DraftThreadController;
	const thread_opens = yield* ThreadOpenController;
	/** A first submission already has its own live reveal and must never sit behind this gate. */
	const draft_handoff = (yield* draft_thread.PendingSubmission(route_id)) !== undefined;
	let thread_open = $state.raw<ThreadOpenSnapshot | undefined>(
		yield* thread_opens.Current(route_id),
	);
	let failure = $state<string | undefined>();
	let loading = $state(untrack(() => thread_open === undefined));
	let visual_settlement = $state.raw<ConversationVisualSettlementMeasurement | undefined>();
	let visually_settled = $state(draft_handoff);

	const RevealThread = (measurement: ConversationVisualSettlementMeasurement) =>
		Effect.gen(function* () {
			if (visually_settled) return;
			visual_settlement = measurement;
			visually_settled = true;
		});

	const Load = Effect.gen(function* () {
		loading = true;
		failure = undefined;
		visual_settlement = undefined;
		visually_settled = draft_handoff;
		yield* thread_opens.Open(route_id).pipe(
			Effect.flatMap((snapshot) =>
				Effect.gen(function* () {
					thread_open = snapshot;
					loading = false;
				}),
			),
			Effect.catch((error) =>
				Effect.gen(function* () {
					failure = error.message;
					loading = false;
				}),
			),
		);
	});

	if (thread_open === undefined) yield* Load.pipe(Effect.forkScoped);
</script>

{#if thread_open !== undefined}
	<div
		class="relative h-full min-h-0"
		aria-busy={!visually_settled}
		data-visual-settlement-duration-ms={visual_settlement?.duration_ms}
		data-visual-settlement-reason={visual_settlement?.reason}
	>
		<!--
			Mount the real route immediately so its transforms and layout can finish,
			but keep it inert and unpainted until the workspace reports stable geometry.
			Removing this cover and opacity happens in the same Svelte commit.
		-->
		<div
			class="h-full min-h-0"
			class:opacity-0={!visually_settled}
			class:pointer-events-none={!visually_settled}
			aria-hidden={!visually_settled}
			inert={!visually_settled}
		>
			<ThreadRoute
				onvisualsettled={draft_handoff ? undefined : RevealThread}
				thread_id={route_id}
				{thread_open}
			/>
		</div>
		{#if !visually_settled}
			<div
				class="absolute inset-0 z-20 flex items-center justify-center"
				aria-label="Loading thread"
				role="status"
			>
				<FadeArc aria-hidden="true" class="size-6 text-muted-foreground" />
			</div>
		{/if}
	</div>
{:else if loading}
	<!--
		A spinner and nothing else. Skeleton bars pretended to know the shape of a
		transcript that had not arrived, and a mocked composer put grey bars where
		a static card was about to be — both read as content failing to appear
		rather than a route still opening. One quiet mark in the middle says
		loading and leaves the surface empty until the real thing paints.
	-->
	<div
		class="flex h-full min-h-0 items-center justify-center"
		aria-label="Loading thread"
		role="status"
	>
		<FadeArc aria-hidden="true" class="size-6 text-muted-foreground" />
	</div>
{:else if failure !== undefined}
	<div class="flex h-full min-h-0 items-center justify-center px-6 text-center">
		<div class="flex max-w-md flex-col items-center gap-3">
			<p class="text-sm text-destructive" role="alert">{failure}</p>
			<button
				type="button"
				class="rounded-md border border-border px-3 py-1.5 text-sm text-foreground hover:bg-muted"
				onclick={yield* Load}
			>
				Retry
			</button>
		</div>
	</div>
{/if}

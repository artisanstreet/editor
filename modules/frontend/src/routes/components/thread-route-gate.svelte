<script lang="ts" effect>
	import { untrack } from "svelte";
	import type { ThreadOpenSnapshot } from "@artisan/protocol";
	import { Effect } from "effect";
	import { FadeArc } from "$lib/components/ui/fade-arc";
	import { ThreadOpenController } from "$lib/thread-interaction/thread-open-controller";
	import ThreadRoute from "./thread-route.svelte";

	let {
		thread_id: route_thread_id,
	}: {
		readonly thread_id: string;
	} = $props();
	const route_id = untrack(() => route_thread_id);
	const thread_opens = yield* ThreadOpenController;
	let thread_open = $state.raw<ThreadOpenSnapshot | undefined>(
		yield* thread_opens.Current(route_id),
	);
	let failure = $state<string | undefined>();
	let loading = $state(untrack(() => thread_open === undefined));

	const Load = Effect.gen(function* () {
		loading = true;
		failure = undefined;
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
	<ThreadRoute thread_id={route_id} {thread_open} />
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

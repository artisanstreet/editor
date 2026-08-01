<script lang="ts" effect>
	import { page } from "$app/state";
	import type { ThreadListItem } from "@artisan/protocol";
	import MessageCircle from "@tabler/icons-svelte/icons/message-circle";
	import { Effect } from "effect";
	import { RunBrowserDom } from "$lib/browser/dom";
	import {
		FormatRecentThreadTime,
		SortRecentThreads,
		ThreadRouteId,
		ThreadRoutePathFor,
	} from "$lib/root/thread-navigation";

	let {
		threads,
	}: {
		/** The live thread list, owned by the layout. */
		threads: ReadonlyArray<ThreadListItem>;
	} = $props();

	/**
	 * The rail reveals on proximity, not on a control: entering the dead margin
	 * is the gesture. Tab focus into a link reveals it the same way, so the
	 * list stays reachable without a pointer.
	 */
	let open = $state(false);
	let zone_element = $state<HTMLDivElement>();
	/** Captured on each reveal so relative times never sit stale on screen. */
	let now_ms = $state(Date.now());

	const recent_threads = $derived(SortRecentThreads(threads));
	const active_route_id = $derived(page.params.thread);

	const Reveal = () =>
		Effect.gen(function* () {
		if (!open) now_ms = Date.now();
		open = true;
		});
	const Conceal = () =>
		Effect.gen(function* () {
		open = false;
		});

	/**
	 * The whole band the list occupies is the trigger, yet it must never block
	 * the transcript beneath it while closed — so the zone is pointer-inert and
	 * proximity is read from the window's pointer position instead. The open
	 * panel re-enables its own pointer events for scrolling and clicking.
	 */
	const TrackPointer = (event: PointerEvent) =>
		Effect.gen(function* () {
		if (zone_element === undefined) return;
		const rect = yield* RunBrowserDom(() => zone_element.getBoundingClientRect());
		const inside =
			event.clientX >= rect.left &&
			event.clientX <= rect.right &&
			event.clientY >= rect.top &&
			event.clientY <= rect.bottom;
		if (inside) yield* Reveal();
		else if (open) yield* Conceal();
		});

</script>

<svelte:window onpointermove={yield* TrackPointer(event)} />

<!--
	The dead margin left of the transcript is the trigger surface itself: resting
	a pointer anywhere in the band the list occupies reveals every thread, and
	leaving lets it fade away. Panel reveal (transitions.dev) on the X axis
	carries the fade — slide, opacity, and cross-blur both ways.
-->
<div
	bind:this={zone_element}
	class="pointer-events-none absolute inset-y-0 left-0 z-10 hidden w-80 xl:block"
	role="presentation"
	onfocusin={yield* Reveal()}
	onfocusout={yield* Conceal()}
>
	<div
		class="t-panel-slide-x absolute inset-0 flex flex-col justify-center py-8 pl-12 pr-2"
		style="--panel-translate-x: -16px"
		data-open={open}
		aria-label="All threads"
	>
		<div
			class="thread-rail-scroll docs-scroll-fade max-h-full overflow-y-auto overflow-x-hidden py-2"
		>
			{#each recent_threads as thread (thread.thread_id)}
				{@const is_active = ThreadRouteId(thread.thread_id) === active_route_id}
				<div class="border-b border-border last:border-b-0">
					<a
						href={ThreadRoutePathFor(thread)}
						class={`block text-sm font-medium outline-none transition-colors duration-(--duration-fast) ease-in-out focus-visible:text-foreground-extra motion-reduce:transition-none ${is_active ? "text-foreground" : "text-muted-foreground hover:text-foreground-extra"}`}
						aria-current={is_active ? "page" : undefined}
					>
						<span class="flex min-w-0 items-center gap-2 py-2.5 pr-2">
							<MessageCircle class="size-4 shrink-0" />
							<span class="min-w-0 flex-1 truncate">{thread.title}</span>
							<span class="whitespace-nowrap text-xs text-muted-foreground">
								{FormatRecentThreadTime(thread.last_activity_at, now_ms)}
							</span>
						</span>
					</a>
				</div>
			{/each}
		</div>
	</div>
</div>

<style>
	/** The home table's scrollbar: thin, muted, and holding its own gutter. */
	.thread-rail-scroll {
		scrollbar-width: thin;
		scrollbar-color: var(--surface-500) transparent;
	}
</style>

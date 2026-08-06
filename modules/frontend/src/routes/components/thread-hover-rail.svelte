<script lang="ts" effect>
	import { page } from "$app/state";
	import type { ThreadListItem } from "@artisan/protocol";
	import { Effect } from "effect";
	import { EngineMarkClass, UsageSlicePresentationFor } from "$lib/engine/presentation";
	import { RunBrowserDom } from "$lib/browser/dom";
	import {
		FormatRecentThreadTime,
		SortRecentThreads,
		ThreadRouteId,
		ThreadRoutePathFor,
	} from "$lib/root/thread-navigation";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";

	let {
		suppressed = false,
		threads,
	}: {
		/**
		 * Set while another surface owns the pointer — an open menu that paints
		 * over this band means a pointer resting here is aimed at that menu, not
		 * at the rail, so proximity stops counting as the gesture.
		 */
		suppressed?: boolean;
		/** The live thread list, owned by the layout. */
		threads: ReadonlyArray<ThreadListItem>;
	} = $props();

	/**
	 * The rail reveals on proximity, not on a control: entering the dead margin
	 * is the gesture. Tab focus into a link reveals it the same way, so the
	 * list stays reachable without a pointer.
	 */
	let near = $state(false);
	/** Suppression wins over proximity, so an overlapping menu hides the rail at once. */
	const open = $derived(near && !suppressed);
	let zone_element = $state<HTMLDivElement>();
	/**
	 * The floating card is wider than the edge strip that summons it, so once
	 * open its own bounds keep the reveal alive — but only once open: closed,
	 * it is an invisible box over the transcript and must never trigger.
	 */
	let card_element = $state<HTMLDivElement>();
	/** Captured on each reveal so relative times never sit stale on screen. */
	let now_ms = $state(Date.now());

	const recent_threads = $derived(SortRecentThreads(threads));
	const active_route_id = $derived(page.params.thread);

	const Reveal = () =>
		Effect.gen(function* () {
		if (suppressed) return;
		if (!near) now_ms = Date.now();
		near = true;
		});
	const Conceal = () =>
		Effect.gen(function* () {
		near = false;
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
		/**
		 * Proximity is dropped rather than frozen while suppressed: a pointer
		 * parked on the menu must not have banked a reveal that fires the
		 * instant the menu closes. Moving again inside the band re-reveals.
		 */
		if (suppressed) {
			yield* Conceal();
			return;
		}
		const rects = yield* RunBrowserDom(() => ({
			card: card_element?.getBoundingClientRect(),
			zone: zone_element.getBoundingClientRect(),
		}));
		const within = (rect: DOMRect | undefined) =>
			rect !== undefined &&
			rect.width > 0 &&
			event.clientX >= rect.left &&
			event.clientX <= rect.right &&
			event.clientY >= rect.top &&
			event.clientY <= rect.bottom;
		/**
		 * The zone summons; the open card only sustains. A closed card's box
		 * sits invisibly over the transcript, so it may never count as inside.
		 */
		const inside = within(rects.zone) || (near && within(rects.card));
		if (inside) yield* Reveal();
		else if (near) yield* Conceal();
		});

</script>

<svelte:window onpointermove={yield* TrackPointer(event)} />

{#snippet thread_rows()}
	{#each recent_threads as thread (thread.thread_id)}
		{@const is_active = ThreadRouteId(thread.thread_id) === active_route_id}
		{@const thread_mark = UsageSlicePresentationFor(thread.engine_id, thread.model_id).mark}
		{@const ThreadMark = thread_mark.icon}
		<div class="border-b border-border last:border-b-0">
			<a
				href={ThreadRoutePathFor(thread)}
				class={`block text-sm font-medium outline-none transition-colors duration-(--duration-fast) ease-in-out focus-visible:text-foreground-extra motion-reduce:transition-none ${is_active ? "text-foreground" : "text-muted-foreground hover:text-foreground-extra"}`}
				aria-current={is_active ? "page" : undefined}
			>
				<span class="flex min-w-0 items-center gap-2 py-2.5 pr-2">
					<!-- The rail names the same thing the list does: what the thread runs on. -->
					<ThreadMark class={EngineMarkClass(thread_mark, "size-4 shrink-0")} />
					<!-- A title that meets its edge blanks out through a mask, the transcript-fade treatment, instead of hitting an ellipsis. -->
					<span class="min-w-0 flex-1 overflow-hidden whitespace-nowrap mask-r-from-85%">{thread.title}</span>
					<span class="whitespace-nowrap text-xs text-muted-foreground @max-[13rem]:hidden">
						{FormatRecentThreadTime(thread.last_activity_at, now_ms)}
					</span>
				</span>
			</a>
		</div>
	{/each}
{/snippet}

<!--
	The dead margin left of the transcript is the trigger surface itself: resting
	a pointer anywhere in the band the list occupies reveals every thread, and
	leaving lets it fade away. Panel reveal (transitions.dev) on the X axis
	carries the fade — slide, opacity, and cross-blur both ways.

	The band is the margin, not a fixed width: the transcript column is a
	centered 48rem, so half of what remains is all the space that truly exists,
	and a wider band would paint rows over the thread itself. It may borrow
	1rem of the column's own inner padding and never grows past its old 20rem.
	The band is a container so the paddings and the timestamp can answer to the
	room it actually got rather than to the window.

	A window too narrow to seat the list in its margin still keeps the gesture:
	the band floors at a thin edge strip, and the reveal floats over the
	transcript as a glass card instead of vanishing with the margin.
-->
<div
	bind:this={zone_element}
	class="@container pointer-events-none absolute inset-y-0 left-0 z-10 w-[clamp(1.5rem,calc((100%_-_48rem)/2_+_1rem),20rem)]"
	role="presentation"
	onfocusin={yield* Reveal()}
	onfocusout={yield* Conceal()}
>
	<!-- With 9rem of band the margin itself seats the list, as it always has. -->
	<div
		class="t-panel-slide-x absolute inset-0 hidden flex-col justify-center py-8 pr-2 pl-[clamp(0.75rem,15cqw,3rem)] @min-[9rem]:flex"
		style="--panel-translate-x: -16px"
		data-open={open}
		aria-label="All threads"
	>
		<div
			class="thread-rail-scroll docs-scroll-fade max-h-full overflow-y-auto overflow-x-hidden py-2"
		>
			{@render thread_rows()}
		</div>
	</div>
	<!--
		Below 9rem the margin can't seat a legible row, so the same list floats
		over the transcript on the standard glass surface. The card is its own
		container, so the timestamp query answers to the card's width rather
		than to the sliver of band that summoned it.
	-->
	<div
		bind:this={card_element}
		class="t-panel-slide-x absolute inset-y-8 left-3 flex w-72 max-w-[calc(100vw-1.5rem)] flex-col justify-center @min-[9rem]:hidden"
		style="--panel-translate-x: -16px"
		data-open={open}
		aria-label="All threads"
	>
		<ShaderGlassSurface strength="strong" class="@container min-h-0 rounded-2xl">
			<div
				class="thread-rail-scroll docs-scroll-fade max-h-full overflow-y-auto overflow-x-hidden px-3 py-2"
			>
				{@render thread_rows()}
			</div>
		</ShaderGlassSurface>
	</div>
</div>

<style>
	/** The home table's scrollbar: thin, muted, and holding its own gutter. */
	.thread-rail-scroll {
		scrollbar-width: thin;
		scrollbar-color: var(--surface-500) transparent;
	}
</style>

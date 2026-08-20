<script lang="ts" effect>
	/**
	 * A map of the conversation down the transcript's right edge.
	 *
	 * At rest it is a column of ticks — one per thing you said, the current one
	 * lit — which says how long the thread is and where you are in it without
	 * spending any width. Pointing at it turns the same ticks into the messages
	 * themselves, so the reader reads the map rather than counting marks.
	 *
	 * The rows are always buttons and always in the document. Collapsing is
	 * purely visual, which keeps the whole map reachable by keyboard and to a
	 * screen reader at every moment, and means hovering reveals the labels of
	 * controls that were already there rather than mounting new ones.
	 */
	import type { Effect } from "effect";
	import type { ConversationTurnMarker } from "$lib/conversation/turn-navigator";
	import DropdownHoverSurface from "./dropdown-hover-surface.svelte";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";

	let {
		active_id,
		markers,
		onselect,
	}: {
		/** The turn the reader is currently at, lit in the column. */
		readonly active_id: string | undefined;
		/** Every turn, oldest first. Empty hides the map entirely. */
		readonly markers: ReadonlyArray<ConversationTurnMarker>;
		readonly onselect: (marker: ConversationTurnMarker) => Effect.Effect<void>;
	} = $props();
</script>

{#if markers.length > 0}
	<!--
		Anchored to the middle of the card rather than the transcript column: it
		belongs to the surface you are reading in, not to the prose, and the prose
		is already offset left of centre to make room for the thread rail.

		`pointer-events-none` on the frame with the rows opting back in keeps the
		collapsed footprint to the ticks themselves, so the invisible expanse the
		panel will occupy never swallows a click meant for the transcript.
	-->
	<!--
		Above the composer's own layer. Both sat on the same one, so the composer
		card — later in the document — painted over the expanded list, cutting off
		exactly the oldest turns the reader opened the list to reach.
	-->
	<nav
		aria-label="Conversation turns"
		class="conversation-range group pointer-events-none absolute top-1/2 right-2 z-30 flex max-h-[70%] max-w-full -translate-y-1/2 justify-end"
	>
		<div class="relative flex min-h-0 max-w-full justify-end">
			<!--
				The environment card's own glass, carried behind the rows rather than
				around them, because this surface has two states and only one of them
				is a card. Wrapping the list would paint a card behind the ticks at
				rest; a backdrop can fade in with the labels it belongs to, and its
				`card-glass` shadow is the same one every other card casts — which is
				what puts a shadow down the right edge here.
			-->
			<!--
				The picker animates a positioning wrapper around its glass, never the
				glass itself. Keeping that boundary here too lets the material recover
				its backdrop as soon as the entrance finishes and `transform` becomes
				`none`.
			-->
			<div aria-hidden="true" class="conversation-range-backdrop pointer-events-none absolute inset-0">
				<ShaderGlassSurface
					class="radius-surface size-full [--radius-gap:var(--spacing)] [--radius-surface:var(--radius-xl)]"
				>
					<span class="sr-only"></span>
				</ShaderGlassSurface>
			</div>
			<!--
				The model picker's scroll grammar: the fade-masked scroller wraps the
				hover surface, so the pill scrolls with the rows it highlights and the
				height bound actually reaches the list — a shrinkless wrapper between
				the two is what let a long thread spill past the glass.
			-->
			<div
				class="docs-scroll-fade pointer-events-auto min-h-0 max-w-full overflow-x-hidden overflow-y-auto"
			>
				<DropdownHoverSurface
					class="flex max-w-full [--docs-sidebar-hover-radius:var(--radius-lg)]"
				>
				{#snippet children({ move_hover })}
					<!--
						Expanded, the map is the inspector's width — the card facing it
						across the transcript — and scrolls once a long thread outgrows the
						frame. Collapsed it is only as wide as a tick, so the pointer keeps
						reaching the transcript underneath.
					-->
					<ol
						class="conversation-range-list flex w-10 max-w-full flex-col items-end gap-1 p-2 group-hover:w-(--inspector-width) group-focus-within:w-(--inspector-width)"
					>
						{#each markers as marker (marker.id)}
							{@const active = marker.id === active_id}
							<li class="w-full">
								<button
									type="button"
									class="relative flex w-full items-center justify-end gap-3 rounded-lg text-left outline-none group-hover:px-3 group-hover:py-1.5 group-focus-within:px-3 group-focus-within:py-1.5 focus-visible:ring-2 focus-visible:ring-ring/50"
									aria-current={active ? "true" : undefined}
									onclick={yield* onselect(marker)}
									onpointerenter={move_hover}
									onpointermove={move_hover}
									onfocusin={move_hover}
								>
									<!--
										The label carries the accessible name at every width, so the
										collapsed column is never a row of unnamed buttons. It ends
										in an ellipsis rather than fading out: a message that fades
										reads as running off the edge of the window, where one that
										ends in "…" reads as a message that was too long.
									-->
									<span
										class={`hidden min-w-0 flex-1 truncate text-sm text-foreground group-hover:block group-focus-within:block ${active ? "font-medium" : ""}`}
									>
										{marker.label}
									</span>
									<span class="sr-only group-hover:hidden group-focus-within:hidden">
										{marker.label}
									</span>
									<!--
										The tick stands in for the row while collapsed. It is the only
										part that carries the reading position at rest, so the current
										one is both brighter and wider than the rest.
									-->
									<span
										aria-hidden="true"
										class={`h-px shrink-0 rounded-full transition-all duration-(--duration-fast) ease-in-out group-hover:hidden group-focus-within:hidden motion-reduce:transition-none ${active ? "w-6 bg-foreground" : "w-4 bg-muted-foreground/50"}`}
									></span>
								</button>
							</li>
						{/each}
					</ol>
				{/snippet}
				</DropdownHoverSurface>
			</div>
		</div>
	</nav>
{/if}

<style>
	/**
	 * Exactly the model picker's dropdown grammar: grow from 0.97 over 250ms,
	 * then leave faster toward 0.99 over 150ms. The right edge is this
	 * surface's trigger, so it replaces bits-ui's floating transform origin.
	 */
	.conversation-range-backdrop {
		opacity: 0;
		transform: scale(var(--dropdown-closing-scale));
		transform-origin: right center;
		transition:
			transform var(--dropdown-close-dur) var(--dropdown-ease),
			opacity var(--dropdown-close-dur) var(--dropdown-ease);
	}

	.conversation-range:hover .conversation-range-backdrop,
	.conversation-range:focus-within .conversation-range-backdrop {
		opacity: 1;
		transform: none;
		animation: conversation-range-in var(--dropdown-open-dur) var(--dropdown-ease);
	}

	/** The card's width opens and closes on the same asymmetric beats. */
	.conversation-range-list {
		transition: width var(--dropdown-close-dur) var(--dropdown-ease);
	}

	.conversation-range:hover .conversation-range-list,
	.conversation-range:focus-within .conversation-range-list {
		transition-duration: var(--dropdown-open-dur);
	}

	@keyframes conversation-range-in {
		from {
			opacity: 0;
			transform: scale(var(--dropdown-pre-scale));
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.conversation-range-backdrop,
		.conversation-range-list {
			animation: none;
			transition: none;
		}
	}
</style>

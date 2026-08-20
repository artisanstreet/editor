<script lang="ts" effect>
	import type { ThreadListItem } from "@artisan/protocol";
	import { Effect, Queue } from "effect";

	import { RunBrowserDom } from "$lib/browser/dom";
	import { RuntimeSurfaceFor } from "$lib/browser/runtime-surface";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import { AttentionMarkedTitle } from "$lib/root/attention-title";
	import { forge_repair_request } from "$lib/root/forge-repair-request.svelte";
	import { ThreadNeedsAttention } from "$lib/root/thread-navigation";

	let {
		threads,
		forge_unreachable = false,
	}: {
		readonly threads: ReadonlyArray<ThreadListItem>;
		/**
		 * True once the renderer has given up on the Forge it was paired with and
		 * proved the origin is gone. Only the shell can obtain a new endpoint, so
		 * this is the ask travelling over the one channel the shell can observe.
		 */
		readonly forge_unreachable?: boolean;
	} = $props();

	const desktop = yield* RunBrowserDom(
		() => RuntimeSurfaceFor(globalThis.navigator?.userAgent ?? "") === "desktop",
	);

	const attention_count = $derived(threads.filter(ThreadNeedsAttention).length);

	/**
	 * The desktop shell always carries a marker — zero included. Its window
	 * title is pinned by the main process, so nobody reads the raw title
	 * there, and an ever-present marker is what makes the channel definite:
	 * the shell never has to guess whether an absent marker means "nothing
	 * needs attention" or "this title predates the marker". A browser tab
	 * shows its title, so it only carries a marker worth showing.
	 */
	const marker_count = $derived(desktop || attention_count > 0 ? attention_count : undefined);

	/**
	 * Repair can be proven (the poll gave up on the paired Forge) or requested
	 * (a surface wants the shell's own Forge back, e.g. the Machine select
	 * returning from a peer). Both ride the same marker.
	 */
	const repair_wanted = $derived(forge_unreachable || forge_repair_request.requested);

	const MarkTitle = (count: number | undefined, requests_repair: boolean) =>
		Effect.gen(function* () {
			yield* RunBrowserDom(() => {
				const marked = AttentionMarkedTitle(document.title, count, requests_repair);
				if (document.title !== marked) document.title = marked;
			});
		});

	/**
	 * Routes own their titles through `svelte:head`, so navigation rewrites
	 * the whole title after this component has marked it. Observing the head
	 * keeps the marker on every route-owned title, exactly as the development
	 * marker does; the idempotent rewrite cannot loop with its own mutations,
	 * and it converges with that marker by always yielding it the very front
	 * of the title.
	 */
	const title_observers = yield* MakeScopedAttachmentRunner(() =>
		Effect.gen(function* () {
			const marks = yield* Queue.unbounded<void>();
			yield* Effect.gen(function* () {
				while (true) {
					yield* Queue.take(marks);
					yield* MarkTitle(marker_count, repair_wanted).pipe(Effect.ignore);
				}
			}).pipe(Effect.forkScoped);
			yield* Queue.offer(marks, undefined);
			yield* Effect.acquireRelease(
				Effect.gen(function* () {
					return yield* RunBrowserDom(() => {
						const observer = new MutationObserver(() => Queue.offerUnsafe(marks, undefined));
						observer.observe(document.head, { characterData: true, childList: true, subtree: true });
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
	yield* title_observers.Replace("attention-title", undefined);

	/**
	 * A thread that settles in the background changes the count with no
	 * navigation and no head mutation, so the marker republishes here the
	 * moment the count moves. Ignored rather than surfaced on failure: during
	 * server rendering there is no document, and a marker that cannot be
	 * written is a marker the next rewrite catches up on.
	 */
	yield* MarkTitle(marker_count, repair_wanted).pipe(Effect.ignore);
</script>

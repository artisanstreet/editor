<script lang="ts" effect>
	import { Effect, Option, Queue } from "effect";

	import { DiscoverForgeHealth } from "$lib/forge/discovery";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import { DevMarkedTitle } from "$lib/root/dev-instance";

	/**
	 * A `/health` body with `development: true` means this renderer is talking
	 * to a development Forge pointed at a separate data root — whether the page
	 * is the built bundle served by that Forge or the HMR dev server proxying
	 * to it. Marking the title removes the standing ambiguity between an
	 * installed app and a repository build, which otherwise look identical and
	 * differ only in which database they read. The title is the whole signal:
	 * a corner badge sat over the shell for the entire session to say something
	 * the window title already says.
	 */
	let development = $state(false);

	const LoadDevelopment = Effect.gen(function* () {
		const health = yield* DiscoverForgeHealth;
		development = Option.getOrUndefined(health)?.development === true;
	});

	yield* LoadDevelopment.pipe(Effect.forkScoped);

	/**
	 * Routes own their titles through `svelte:head`, so navigation rewrites the
	 * whole title after this component has marked it. Observing the head keeps
	 * the `[Dev]` marker on every route-owned title for as long as the page is
	 * known to face a development Forge; the idempotent prefix cannot loop with
	 * its own mutations.
	 */
	const title_observers = yield* MakeScopedAttachmentRunner(() =>
		Effect.gen(function* () {
			const marks = yield* Queue.unbounded<void>();
			const MarkTitle = Effect.gen(function* () {
				yield* RunBrowserDom(() => {
					const marked = DevMarkedTitle(document.title);
					if (document.title !== marked) document.title = marked;
				});
			});
			yield* Effect.gen(function* () {
				while (true) {
					yield* Queue.take(marks);
					yield* MarkTitle;
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
	const SyncTitleObserver = Effect.gen(function* () {
		if (development) {
			yield* title_observers.Replace("development-title", undefined);
			return;
		}
		yield* title_observers.Release("development-title");
	});
	yield* SyncTitleObserver;
</script>

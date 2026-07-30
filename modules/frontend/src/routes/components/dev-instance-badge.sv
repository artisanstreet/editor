<script lang="ts" effect>
	import { Effect, Option } from "effect";

	import { DiscoverForgeHealth } from "$lib/forge/discovery";
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

	yield* DiscoverForgeHealth.pipe(
		Effect.flatMap((health) =>
			Effect.sync(() => {
				development = Option.getOrUndefined(health)?.development === true;
			}),
		),
		Effect.forkScoped,
	);

	/**
	 * Routes own their titles through `svelte:head`, so navigation rewrites the
	 * whole title after this component has marked it. Observing the head keeps
	 * the `[Dev]` marker on every route-owned title for as long as the page is
	 * known to face a development Forge; the idempotent prefix cannot loop with
	 * its own mutations.
	 */
	$effect(() => {
		if (!development) return;

		const Mark = () => {
			const marked = DevMarkedTitle(document.title);
			if (document.title !== marked) document.title = marked;
		};

		Mark();
		const observer = new MutationObserver(Mark);
		observer.observe(document.head, { characterData: true, childList: true, subtree: true });
		return () => observer.disconnect();
	});
</script>

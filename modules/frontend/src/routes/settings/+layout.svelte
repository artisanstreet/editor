<script lang="ts" effect>
	import { page } from "$app/state";
	import { Effect } from "effect";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { TooltipProvider } from "$lib/components/ui/tooltip";
	import SettingsNav from "../components/settings/nav.svelte";

	let { children } = $props();

	/** `/settings/section#header` deep links land on their header. */
	const ScrollToHash = Effect.gen(function* () {
		const hash = page.url.hash.slice(1);
		if (hash === "") return;
		yield* RunBrowserDom(() => document.getElementById(hash)?.scrollIntoView({ block: "start" }));
	});
	if (page.url.hash !== "") yield* ScrollToHash;
</script>

<TooltipProvider delayDuration={0} ignoreNonKeyboardFocus>
<div class="flex h-full min-h-0 justify-center">
	<div class="flex w-full max-w-4xl gap-10 px-6">
		<div class="sticky top-0 shrink-0 self-start py-12">
			<SettingsNav />
		</div>
		<div class="min-w-0 grow overflow-y-auto py-12 [scrollbar-width:thin]">
			{@render children()}
		</div>
	</div>
</div>
</TooltipProvider>

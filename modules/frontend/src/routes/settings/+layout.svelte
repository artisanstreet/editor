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

	const section_path = $derived(page.url.pathname);
</script>

<TooltipProvider delayDuration={0} ignoreNonKeyboardFocus>
	<div class="h-full min-h-0 overflow-y-auto [scrollbar-width:thin]">
		<div class="mx-auto flex min-h-full w-full max-w-4xl flex-col px-4 md:flex-row md:gap-14 md:px-6">
			<aside
				class="sticky top-0 z-20 -mx-4 shrink-0 border-b border-border/40 bg-background/95 px-4 pt-5 pb-3 backdrop-blur-xl md:z-auto md:mx-0 md:w-44 md:self-start md:border-b-0 md:bg-transparent md:px-0 md:py-12 md:backdrop-blur-none"
			>
				<p class="mb-3 px-2 text-sm font-semibold tracking-tight text-foreground md:mb-5">
					Settings
				</p>
				<SettingsNav />
			</aside>
			<main class="min-w-0 grow pt-8 pb-12 md:py-12">
				<!--
					Keyed on the pathname so each section arrives with one quiet rise;
					hash-only navigation stays put and just scrolls.
				-->
				{#key section_path}
					<div class="animate-[settings-page-enter_var(--duration-fast)_var(--ease-smooth-out)_both]">
						{@render children()}
					</div>
				{/key}
			</main>
		</div>
	</div>
</TooltipProvider>

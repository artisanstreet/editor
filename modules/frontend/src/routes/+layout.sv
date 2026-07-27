<script lang="ts" effect>
	import "$lib/styles/fonts.css";
	import "$lib/styles/global.css";
	import "$lib/styles/artisan-compatibility.css";

	import { page } from "$app/state";
	import {
		ArtisanClient,
		type ArtisanConnectionState,
	} from "@artisan/transport/client";
	import { Effect, Stream } from "effect";
	import { ModeWatcher } from "mode-watcher";
	import { onMount } from "svelte";
	import { Toaster } from "svelte-sonner";
	import { BannerService } from "$lib/banner/service";
	import ArtisanSidebar from "./components/artisan-sidebar.sv";
	import ForgeConnectionBanner from "./components/forge-connection-banner.sv";
	import SectionedPanel from "./components/sectioned-panel.sv";
	import ThreadPanel from "./components/thread-panel.sv";

	let { children } = $props();
	let desktop_runtime = $state(false);
	let forge_ready = $state(false);
	const is_thread = $derived(/^\/threads\/[^/]+\/?$/.test(page.url.pathname));
	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	const hydration_banner_id = "forge-hydration";

	const HydrateForge = (): Effect.Effect<void> =>
		Effect.sync(() => {
			forge_ready = false;
		}).pipe(
			Effect.andThen(
				Effect.all([client.GetRuntimeCatalog, client.ListProjects, client.ListThreads], {
					concurrency: "unbounded",
					discard: true,
				}),
			),
			Effect.andThen(client.ConnectionState),
			Effect.flatMap((state) =>
				state.phase === "ready"
					? Effect.sync(() => {
							forge_ready = true;
						})
					: Effect.void,
			),
			Effect.andThen(banner.dismiss(hydration_banner_id)),
			Effect.catch((error) =>
				banner.error("Could not hydrate Forge", {
					actions: [
						{
							Execute: HydrateForge(),
							icon: "refresh",
							id: "retry-hydration",
							label: "Retry now",
						},
					],
					code: "forge.hydration.failed",
					description: error.message,
					duration_ms: Number.POSITIVE_INFINITY,
					id: hydration_banner_id,
				}),
			),
		);

	const ApplyConnectionState = (state: ArtisanConnectionState) =>
		state.phase === "ready"
			? HydrateForge()
			: Effect.sync(() => {
					forge_ready = false;
				});

	yield* client.ConnectionState.pipe(Effect.flatMap(ApplyConnectionState));
	yield* client.ConnectionChanges.pipe(
		Stream.runForEach(ApplyConnectionState),
		Effect.forkScoped,
	);

	onMount(() => {
		desktop_runtime = navigator.userAgent.includes("Electron/");
	});
</script>

<ModeWatcher defaultMode="dark" />
<Toaster position="top-center" />
<ForgeConnectionBanner />

{#snippet sidebar()}
	<ArtisanSidebar />
{/snippet}

{#snippet primary()}
	{@render children()}
{/snippet}

{#snippet secondary()}
	<ThreadPanel />
{/snippet}

<div class="flex h-dvh min-h-0 flex-col bg-background">
	{#if desktop_runtime}
		<div
			aria-hidden="true"
			class="h-10 shrink-0 bg-background"
			style="-webkit-app-region: drag;"
		></div>
	{/if}
	{#if forge_ready}
		<div class="min-h-0 flex-1">
			<SectionedPanel {sidebar} {primary} secondary={is_thread ? secondary : undefined} />
		</div>
	{/if}
</div>

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
	import {
		BeginForgeHydration,
		CompleteForgeHydration,
		FailForgeHydration,
		InitialForgeGateModel,
		ObserveForgeConnection,
	} from "$lib/forge/gate";
	import ArtisanSidebar from "./components/artisan-sidebar.sv";
	import ForgeConnectionOverlay from "./components/forge-connection-overlay.sv";
	import ForgeShellPreview from "./components/forge-shell-preview.sv";
	import SectionedPanel from "./components/sectioned-panel.sv";
	import ThreadPanel from "./components/thread-panel.sv";

	let { children } = $props();
	let desktop_runtime = $state(false);
	let forge_gate = $state.raw(InitialForgeGateModel);
	const is_thread = $derived(/^\/threads\/[^/]+\/?$/.test(page.url.pathname));
	const client = yield* ArtisanClient;

	const HydrateForge = (generation: number): Effect.Effect<void> =>
		Effect.all([client.GetRuntimeCatalog, client.ListProjects, client.ListThreads], {
			concurrency: "unbounded",
			discard: true,
		}).pipe(
			Effect.andThen(client.ConnectionState),
			Effect.flatMap((state) =>
				state.phase === "ready"
					? Effect.sync(() => {
							forge_gate = CompleteForgeHydration(forge_gate, generation);
						})
					: ApplyConnectionState(state),
			),
			Effect.catch((error) =>
				Effect.sync(() => {
					forge_gate = FailForgeHydration(forge_gate, generation, error.message);
				}),
			),
		);

	const ApplyConnectionState = (
		state: ArtisanConnectionState,
	): Effect.Effect<void> =>
		Effect.sync(() => {
			forge_gate = ObserveForgeConnection(forge_gate, state);
			return forge_gate;
		}).pipe(
			Effect.flatMap((model) =>
				model.state.phase === "hydrating"
					? HydrateForge(model.state.generation).pipe(
							Effect.forkScoped,
							Effect.asVoid,
						)
					: Effect.void,
			),
		);

	const RetryHydration = Effect.sync(() => {
		forge_gate = BeginForgeHydration(forge_gate);
		return forge_gate.hydration_generation;
	}).pipe(Effect.flatMap(HydrateForge));

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
	<div class="relative min-h-0 flex-1">
		<div
			class="h-full"
			inert={forge_gate.state.phase !== "ready"}
		>
			{#if forge_gate.has_hydrated_shell}
				<SectionedPanel {sidebar} {primary} secondary={is_thread ? secondary : undefined} />
			{:else}
				<ForgeShellPreview />
			{/if}
		</div>
		<ForgeConnectionOverlay
			model={forge_gate}
			retry_connection={client.RetryConnection}
			retry_hydration={RetryHydration}
		/>
	</div>
</div>

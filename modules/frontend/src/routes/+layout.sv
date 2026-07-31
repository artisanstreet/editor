<script lang="ts" effect>
	import "$lib/styles/fonts.css";
	import "$lib/styles/global.css";
	import "$lib/styles/artisan-compatibility.css";

	import { page } from "$app/state";
	import type { ThreadListItem } from "@artisan/protocol";
	import {
		ArtisanClient,
		type ArtisanConnectionState,
		type ThreadListUpdate,
	} from "@artisan/transport/client";
	import { Effect, Option, Stream } from "effect";
	import { ModeWatcher } from "mode-watcher";
	import { onMount } from "svelte";
	import { Toaster } from "svelte-sonner";
	import {
		BeginForgeHydration,
		CompleteForgeHydration,
		DismissForgeGate,
		FailForgeHydration,
		ForgeShellIsBlocked,
		ForgeShellIsMounted,
		InitialForgeGateModel,
		ObserveForgeConnection,
	} from "$lib/forge/gate";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import { EditorWorkspaceId } from "$lib/editor/workspace-identity";
	import { draft_thread_project } from "$lib/root/draft-thread";
	import {
		ApplyRootThreadListUpdate,
		ResolveThreadRoute,
	} from "$lib/root/thread-navigation";
	import { ForgeHttpUrl } from "$lib/runtime/forge-endpoint";
	import { AttemptDevelopmentSelfPair } from "$lib/runtime/pairing";
	import DevInstanceBadge from "./components/dev-instance-badge.sv";
	import ForgeConnectionOverlay from "./components/forge-connection-overlay.sv";
	import ForgeShellPreview from "./components/forge-shell-preview.sv";
	import EditorFilePanel from "./components/editor-file-panel.sv";
	import SectionedPanel from "./components/sectioned-panel.sv";
	import ThreadPanel from "./components/thread-panel.sv";

	let { children } = $props();
	let desktop_runtime = $state(false);
	let forge_gate = $state.raw(InitialForgeGateModel);
	let threads = $state.raw<ReadonlyArray<ThreadListItem>>([]);
	/** The draft, compatibility entry point, and canonical thread route own the inspector. */
	const is_thread = $derived(
		/^\/t\/[^/]+\/[^/]+\/?$/.test(page.url.pathname) ||
			/^\/threads(?:\/[^/]+)?\/?$/.test(page.url.pathname),
	);
	/**
	 * The layout owns route-derived state and hands it down: read inside the panel
	 * itself, the same derivation went stale after a client-side navigation.
	 */
	const surface = $derived(
		page.url.pathname.startsWith("/e/") || page.url.pathname.startsWith("/editor")
			? "editor"
			: "threads",
	);
	const active_route_thread_id = $derived(page.params.thread ?? page.params.id);
	const active_thread = $derived(
		active_route_thread_id === undefined
			? undefined
			: Option.getOrUndefined(ResolveThreadRoute(threads, active_route_thread_id)),
	);
	/**
	 * The workspace the current route is actually inside, or nothing. The open
	 * thread names its project, the draft names the project picked for it, and
	 * the editor names its own `?workspace=` — there is no fallback to "some
	 * attached project", so on routes outside any workspace this stays closed.
	 */
	const active_workspace_id = $derived.by(() => {
		if (active_thread !== undefined) return active_thread.primary_project?.project_id;
		if (surface === "editor")
			return EditorWorkspaceId(page.url.searchParams.get("workspace") ?? undefined);
		if (is_thread) return $draft_thread_project?.project_id;
		return undefined;
	});
	const client = yield* ArtisanClient;

	const ApplyThreadListUpdate = (update: ThreadListUpdate) =>
		Effect.sync(() => {
			threads = ApplyRootThreadListUpdate(threads, update);
		});

	const RefreshThreads = client.ListThreads.pipe(
		Effect.map((next_threads) => ({
			journal_sequence: 0,
			threads: next_threads,
			type: "snapshot" as const,
		})),
		Effect.flatMap(ApplyThreadListUpdate),
	);

	/**
	 * The shell mounts before Forge is necessarily reachable, and the
	 * subscription below re-runs this same refresh on every recovery attempt.
	 * A failed first load is therefore expected and already answered.
	 */
	yield* RefreshThreads.pipe(Effect.ignore);

	yield* RunAuthoritativeSubscription(
		client.SubscribeThreadList,
		ApplyThreadListUpdate,
		RefreshThreads,
	).pipe(Effect.forkScoped);

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

	/**
	 * Dismissing the gate hands the shell over in its disconnected state so the
	 * interface itself can be read and exercised. Reconnection keeps running
	 * underneath, and every surface resubscribes on its own backoff, so the
	 * workspace fills in the moment Forge returns.
	 */
	const DismissGate = () => {
		forge_gate = DismissForgeGate(forge_gate);
	};

	const RetryHydration = Effect.sync(() => {
		forge_gate = BeginForgeHydration(forge_gate);
		return forge_gate.hydration_generation;
	}).pipe(Effect.flatMap(HydrateForge));

	yield* client.ConnectionChanges.pipe(
		Stream.runForEach(ApplyConnectionState),
		Effect.forkScoped,
	);

	/**
	 * The transport parks after its reconnect budget, which a Forge restart
	 * (notably an update) always exhausts. This watcher probes the origin while
	 * the gate is parked and re-arms the connection once — so a returning Forge
	 * reconnects by itself, and the gate's "isn't paired" diagnosis is only ever
	 * shown after a real attempt against a reachable Forge has failed.
	 */
	const AutoRecoverConnection = Effect.gen(function* () {
		let attempted = false;
		while (true) {
			yield* Effect.sleep("2 seconds");
			if (forge_gate.state.phase !== "exhausted") {
				attempted = false;
				continue;
			}
			if (attempted) continue;
			const reachable = yield* Effect.tryPromise(() =>
				fetch(ForgeHttpUrl("/health"), { cache: "no-store" }).then(
					(response) => response.ok,
				),
			).pipe(Effect.catch(() => Effect.succeed(false)));
			if (!reachable || forge_gate.state.phase !== "exhausted") continue;
			/**
			 * A reachable Forge with an exhausted connection is the unpaired
			 * symptom in browser development; self-pair over the dev server's
			 * same-origin endpoint before retrying so no terminal is needed.
			 */
			if (import.meta.env.DEV) {
				yield* AttemptDevelopmentSelfPair;
			}
			attempted = true;
			yield* client.RetryConnection;
		}
	});
	yield* AutoRecoverConnection.pipe(Effect.forkScoped);

	onMount(() => {
		desktop_runtime = navigator.userAgent.includes("Electron/");
	});
</script>

<ModeWatcher defaultMode="dark" />
<Toaster position="top-center" />
<DevInstanceBadge />

{#snippet primary()}
	{@render children()}
{/snippet}

{#snippet secondary()}
	<ThreadPanel />
{/snippet}

<!--
	The editor's file tree takes the same column the thread inspector uses: one
	surface belongs to whatever workspace is open, rather than each surface
	inventing its own place to put things.
-->
{#snippet editor_files()}
	<EditorFilePanel />
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
			inert={ForgeShellIsBlocked(forge_gate)}
		>
			{#if ForgeShellIsMounted(forge_gate)}
				<SectionedPanel
					{primary}
					{surface}
					{threads}
					thread_id={active_thread?.thread_id}
					workspace_id={active_workspace_id}
					secondary={surface === "editor" ? editor_files : is_thread ? secondary : undefined}
				/>
			{:else}
				<ForgeShellPreview />
			{/if}
		</div>
		<ForgeConnectionOverlay
			model={forge_gate}
			ondismiss={DismissGate}
			retry_connection={client.RetryConnection}
			retry_hydration={RetryHydration}
		/>
	</div>
</div>

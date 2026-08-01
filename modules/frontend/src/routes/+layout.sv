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
	import * as HttpClient from "effect/unstable/http/HttpClient";
	import { ModeWatcher } from "mode-watcher";
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
	import { RunBrowserDom } from "$lib/browser/dom";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import {
		DraftThreadController,
		type DraftThreadState,
	} from "$lib/root/draft-thread";
	import {
		ApplyRootThreadListUpdate,
		ResolveThreadRoute,
	} from "$lib/root/thread-navigation";
	import { ForgeHttpUrl } from "$lib/runtime/forge-endpoint";
	import { AttemptDevelopmentSelfPair } from "$lib/runtime/pairing";
	import { SessionDefaultsController } from "$lib/settings/session-defaults-controller";
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
	const draft_thread = yield* DraftThreadController;
	let draft_state = $state.raw<DraftThreadState>(yield* draft_thread.Current);
	const ApplyDraftState = (next: DraftThreadState) =>
		Effect.gen(function* () {
			draft_state = next;
		});
	yield* draft_thread.Changes.pipe(
		Stream.runForEach(ApplyDraftState),
		Effect.forkScoped,
	);
	/** Only a canonical conversation route owns the transcript proximity rail. */
	const is_thread_route = $derived(
		/^\/t\/[^/]+\/[^/]+\/?$/.test(page.url.pathname),
	);
	/** The root draft and canonical thread route own the inspector. */
	const is_thread = $derived(is_thread_route || page.url.pathname === "/");
	/**
	 * The layout owns route-derived state and hands it down: read inside the panel
	 * itself, the same derivation went stale after a client-side navigation.
	 */
	const surface = $derived(page.url.pathname.startsWith("/e/") ? "editor" : "threads");
	const active_route_thread_id = $derived(page.params.thread);
	const active_thread = $derived(
		active_route_thread_id === undefined
			? undefined
			: Option.getOrUndefined(ResolveThreadRoute(threads, active_route_thread_id)),
	);
	/**
	 * The workspace the current route is actually inside, or nothing. The open
	 * thread names its project and the draft names the project picked for it.
	 * There is no fallback to a route-asserted or arbitrary attached project, so
	 * routes outside an authoritative workspace keep the surface switch closed.
	 */
	const active_workspace_id = $derived.by(() => {
		if (active_thread !== undefined) return active_thread.primary_project?.project_id;
		if (is_thread && draft_state._tag !== "Uninitialized") {
			return draft_state.project?.project_id;
		}
		return undefined;
	});
	const client = yield* ArtisanClient;
	const session_defaults = yield* SessionDefaultsController;

	const ApplyThreadListUpdate = (update: ThreadListUpdate) =>
		Effect.gen(function* () {
			threads = ApplyRootThreadListUpdate(threads, update);
		});

	const RefreshThreads = Effect.gen(function* () {
		const next_threads = yield* client.ListThreads;
		yield* ApplyThreadListUpdate({
			journal_sequence: 0,
			threads: next_threads,
			type: "snapshot" as const,
		});
	});

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
		Effect.gen(function* () {
			yield* Effect.all(
				[session_defaults.Refresh, client.ListProjects, client.ListThreads],
				{
					concurrency: "unbounded",
					discard: true,
				},
			);
			const state = yield* client.ConnectionState;
			if (state.phase === "ready") {
				forge_gate = CompleteForgeHydration(forge_gate, generation);
				return;
			}
			yield* ApplyConnectionState(state);
		}).pipe(
			Effect.catch((error) =>
				Effect.gen(function* () {
					forge_gate = FailForgeHydration(forge_gate, generation, error.message);
				}),
			),
		);

	const ApplyConnectionState = (
		state: ArtisanConnectionState,
	): Effect.Effect<void> =>
		Effect.gen(function* () {
			forge_gate = ObserveForgeConnection(forge_gate, state);
			if (forge_gate.state.phase !== "hydrating") return;
			yield* HydrateForge(forge_gate.state.generation).pipe(Effect.forkScoped);
		});

	/**
	 * Dismissing the gate hands the shell over in its disconnected state so the
	 * interface itself can be read and exercised. Reconnection keeps running
	 * underneath, and every surface resubscribes on its own backoff, so the
	 * workspace fills in the moment Forge returns.
	 */
	const DismissGate = Effect.gen(function* () {
		forge_gate = DismissForgeGate(forge_gate);
	});

	const RetryHydration = Effect.gen(function* () {
		forge_gate = BeginForgeHydration(forge_gate);
		yield* HydrateForge(forge_gate.hydration_generation);
	});

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
		const http_client = yield* HttpClient.HttpClient;
		let attempted = false;
		while (true) {
			yield* Effect.sleep("2 seconds");
			if (forge_gate.state.phase !== "exhausted") {
				attempted = false;
				continue;
			}
			if (attempted) continue;
			const reachable = yield* http_client
				.get(yield* ForgeHttpUrl("/health"))
				.pipe(
					Effect.map((response) => response.status >= 200 && response.status < 300),
					Effect.catch(() =>
						Effect.gen(function* () {
							return false;
						}),
					),
				);
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

	desktop_runtime = yield* RunBrowserDom(
		() => globalThis.navigator?.userAgent.includes("Electron/") ?? false,
	);
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
					show_thread_hover_rail={is_thread_route}
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

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
		ForgeHydrationFailure,
		ForgeShellIsBlocked,
		ForgeShellIsMounted,
		InitialForgeGateModel,
		IsCurrentForgeHydration,
		ObserveForgeConnection,
	} from "$lib/forge/gate";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import { LogTransportDiagnostic } from "$lib/forge/diagnostics";
	import {
		DraftThreadController,
		type DraftThreadState,
	} from "$lib/root/draft-thread";
	import {
		ApplyRootThreadListUpdate,
		ResolveThreadRoute,
	} from "$lib/root/thread-navigation";
	import { ForgeHttpUrl } from "$lib/runtime/forge-endpoint";
	import { prose_width, shader_enabled } from "$lib/appearance-config";
	import { AppearancePreferences } from "$lib/runtime/appearance-preferences";
	import { AttemptDevelopmentSelfPair } from "$lib/runtime/pairing";
	import { SessionDefaultsController } from "$lib/settings/session-defaults-controller";
	import DevInstanceBadge from "./components/dev-instance-badge.svelte";
	import ForgeConnectionOverlay from "./components/forge-connection-overlay.svelte";
	import ForgeShellPreview from "./components/forge-shell-preview.svelte";
	import EditorFilePanel from "./components/editor-file-panel.svelte";
	import SectionedPanel from "./components/sectioned-panel.svelte";
	import ThreadPanel from "./components/thread-panel.svelte";
	import WorkspaceHeader from "./components/workspace-header.svelte";

	let { children } = $props();
	let desktop_runtime = $state(false);
	let mac_desktop = $state(false);
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
	const active_project = $derived.by(() => {
		if (active_thread !== undefined) return active_thread.primary_project;
		if (is_thread && draft_state._tag !== "Uninitialized") return draft_state.project;
		return undefined;
	});
	const active_workspace_id = $derived(active_project?.project_id);
	/**
	 * Only a durable thread titles itself with its workspace. The root draft
	 * also knows a picked project, but nothing exists there yet to title — a
	 * header on `/` would name a folder above an empty composer.
	 */
	const header_project = $derived(active_thread?.primary_project);
	const client = yield* ArtisanClient;
	const session_defaults = yield* SessionDefaultsController;

	/**
	 * Read once for the whole shell: every glass surface reads the store while
	 * rendering, so the stored preference has to reach it here rather than when
	 * the settings screen happens to be opened.
	 */
	const appearance = yield* AppearancePreferences;
	const stored_appearance = yield* appearance.Load;
	shader_enabled.set(stored_appearance.shader_enabled);
	prose_width.set(stored_appearance.prose_width ?? "balanced");

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
			const [, , next_threads] = yield* Effect.all(
				[
					session_defaults.Refresh.pipe(
						Effect.mapError(
							(error) =>
								new ForgeHydrationFailure({ error, operation: "session_defaults" }),
						),
					),
					client.ListProjects.pipe(
						Effect.mapError(
							(error) => new ForgeHydrationFailure({ error, operation: "projects" }),
						),
					),
					client.ListThreads.pipe(
						Effect.mapError(
							(error) => new ForgeHydrationFailure({ error, operation: "threads" }),
						),
					),
				],
				{
					concurrency: "unbounded",
				},
			);
			const state = yield* client.ConnectionState;
			if (state.phase === "ready") {
				if (!IsCurrentForgeHydration(forge_gate, generation)) return;
				yield* ApplyThreadListUpdate({
					journal_sequence: 0,
					threads: next_threads,
					type: "snapshot" as const,
				});
				forge_gate = CompleteForgeHydration(forge_gate, generation);
				return;
			}
			yield* ApplyConnectionState(state);
		}).pipe(
			Effect.catchTag("ForgeHydrationFailure", ({ error, operation }) =>
				Effect.gen(function* () {
					forge_gate = FailForgeHydration(forge_gate, generation, operation, error);
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
	 * The transport journal reaches the devtools console continuously, not just
	 * when an overlay is on screen — the interesting entries are the ones
	 * recorded minutes before a failure finally surfaces.
	 */
	yield* client.DiagnosticEvents.pipe(
		Stream.runForEach(LogTransportDiagnostic),
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
	/**
	 * macOS is the one desktop platform whose window controls overlay the
	 * strip's left end instead of its right, exactly where the left-anchored
	 * identity would sit. The identity swaps sides there rather than padding
	 * itself past the traffic lights: a right-aligned line reads cleanly and
	 * the lights keep their native position.
	 */
	mac_desktop = yield* RunBrowserDom(
		() =>
			(globalThis.navigator?.userAgent.includes("Electron/") ?? false) &&
			(globalThis.navigator?.userAgent.includes("Macintosh") ?? false),
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

{#snippet workspace_header()}
	<WorkspaceHeader project={header_project} thread_title={active_thread?.title} />
{/snippet}

<div class="flex h-dvh min-h-0 flex-col bg-background" data-prose-width={$prose_width}>
	{#if desktop_runtime}
		<!--
			The bundled shell hides the native titlebar, so this strip is the window
			frame: it drags the window and the overlay controls float over its right
			end. The identity anchors to the primary card's left edge — not the
			prose column's — because in a wide window a prose-anchored title floats
			out into the middle of the frame with nothing under it to explain the
			offset. The spacers replay the shell row underneath: sidebar rail,
			then the inspector column and the gaps around it whenever a workspace
			inspector is open. The identity carries no left inset of its own: its
			first glyph sits exactly on the card edge the rail spacer lands on.
			On macOS the traffic lights occupy that left end instead, so the
			identity right-aligns against the inspector edge and leaves the
			lights' corner empty; the left inset there guards the narrow-window
			case where a long line would otherwise truncate into them.
		-->
		<div
			class="flex h-10 shrink-0 items-stretch bg-background"
			style="-webkit-app-region: drag;"
		>
			<div class="w-14 shrink-0" aria-hidden="true"></div>
			<div class="flex min-w-0 flex-1 items-center pr-1">
				<!--
					The strip stays while the gate overlay is up — it is still the window
					frame — but the identity line goes: the overlay covers only the shell
					area below, and a workspace title floating over a connection-failure
					screen names a workspace the screen says is unreachable.
				-->
				{#if !ForgeShellIsBlocked(forge_gate)}
					<div
						class={mac_desktop
							? "flex w-full min-w-0 items-center justify-end pl-16 pr-6"
							: "flex w-full min-w-0 items-center pr-6"}
					>
						{@render workspace_header()}
					</div>
				{/if}
			</div>
			{#if surface === "editor" || is_thread}
				<div
					class="w-[calc(clamp(16rem,25vw,350px)+1rem)] shrink-0"
					aria-hidden="true"
				></div>
			{:else}
				<div class="w-2 shrink-0" aria-hidden="true"></div>
			{/if}
		</div>
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
					header={desktop_runtime || header_project === undefined
						? undefined
						: workspace_header}
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
			read_diagnostics={client.Diagnostics}
			retry_connection={client.RetryConnection}
			retry_hydration={RetryHydration}
		/>
	</div>
</div>

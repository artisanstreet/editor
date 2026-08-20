<script lang="ts" effect>
	import "$lib/styles/fonts.css";
	import "$lib/styles/global.css";

	import { page } from "$app/state";
	import type { EventEnvelope } from "@artisan/protocol";
	import { ArtisanClient, type ArtisanConnectionState } from "@artisan/transport/client";
	import { Effect, Option, Stream } from "effect";
	import * as HttpClient from "effect/unstable/http/HttpClient";
	import { ModeWatcher } from "mode-watcher";
	import {
		BeginForgeHydration,
		CompleteForgeHydration,
		DismissForgeGate,
		ForgeShellIsBlocked,
		ForgeShellIsMounted,
		InitialForgeGateModel,
		IsCurrentForgeHydration,
		ObserveForgeConnection,
	} from "$lib/forge/gate";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { WarmConversationRenderers } from "$lib/components/markdown/warming";
	import { BrowserReaderAttention } from "$lib/browser/reader-attention";
	import { IsMacDesktop, RuntimeSurfaceFor } from "$lib/browser/runtime-surface";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import {
		ThreadChecklist,
		conversation_plan_has_open_entries,
		type ThreadChecklistState,
	} from "$lib/conversation/checklist";
	import {
		ThreadOrchestrationRoster,
		type ThreadOrchestrationState,
	} from "$lib/orchestration/service";
	import { LogTransportDiagnostic } from "$lib/forge/diagnostics";
	import { ProbeForgeRecoveryHealth } from "$lib/forge/recovery-health-probe";
	import { IsNotifiableEvent } from "$lib/notifications/events";
	import { SystemNotifications } from "$lib/notifications/service";
	import {
		ResolveThreadRoute,
		ThreadRoutePathFor,
		ThreadWorkspaceId,
	} from "$lib/root/thread-navigation";
	import {
		WorkspaceCatalogController,
		type WorkspaceCatalogState,
	} from "$lib/root/workspace-catalog-controller";
	import { ForgeHttpUrl } from "$lib/runtime/forge-endpoint";
	import { resolve_typography_preferences } from "$lib/appearance/typography";
	import {
		prose_width,
		shader_enabled,
		typography,
	} from "$lib/appearance-config";
	import { BrowserTypography } from "$lib/browser/typography";
	import {
		AppearancePreferences,
		type AppearanceState,
	} from "$lib/runtime/appearance-preferences";
	import { AttemptDevelopmentSelfPair } from "$lib/runtime/pairing";
	import { SessionDefaultsController } from "$lib/settings/session-defaults-controller";
	import AttentionTitleMarker from "./components/attention-title-marker.svelte";
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
	let forge_unreachable = $state(false);
	const client = yield* ArtisanClient;
	const session_defaults = yield* SessionDefaultsController;
	const workspace_catalog = yield* WorkspaceCatalogController;
	let catalog_state = $state.raw<WorkspaceCatalogState>(yield* workspace_catalog.Current);
	const threads = $derived(catalog_state.threads);
	const threads_loaded = $derived(catalog_state.threads_loaded);
	/**
	 * The attached catalog, held by the shell because the shell is what has to
	 * turn a `/t/<workspace>` segment back into a project. A route can assert an
	 * identity; only Forge can say it exists.
	 */
	const projects = $derived(catalog_state.projects);
	/** A canonical conversation: a workspace and a thread inside it. */
	const is_conversation_route = $derived(
		/^\/t\/[^/]+\/[^/]+\/?$/.test(page.url.pathname),
	);
	/** The workspace itself, with no thread open in it yet. */
	const is_workspace_route = $derived(/^\/t\/[^/]+\/?$/.test(page.url.pathname));
	/**
	 * Every surface a thread is started from: the root, which names its project
	 * in prose, and a workspace, which names it in the URL. They are one surface
	 * with two ways of answering the same question.
	 */
	const is_new_thread_route = $derived(page.url.pathname === "/" || is_workspace_route);
	/**
	 * Both `/t/` surfaces. They are the same room — the transcript's left margin
	 * carries the thread list in either, and the workspace route is simply the
	 * one where the transcript has not been written yet.
	 */
	const is_thread_route = $derived(is_conversation_route || is_workspace_route);
	/**
	 * Every thread surface carries the list the same way: the margin stays clear
	 * until the pointer asks for it. Routes outside a thread surface do not carry
	 * it at all.
	 */
	const thread_rail = $derived(
		is_conversation_route || is_new_thread_route ? "proximity" : "hidden",
	);
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
	 * The workspace the current route is actually inside, or nothing.
	 *
	 * The open thread names its project authoritatively. Failing that, a `/t/`
	 * route names one in the URL, and it is resolved against the catalog rather
	 * than trusted: a segment naming a project Forge does not have is not a
	 * workspace, and the surfaces keyed on this must not act as though it were.
	 * There is no fallback to an arbitrary attached project.
	 */
	const route_workspace_id = $derived(
		is_thread_route ? ThreadWorkspaceId(page.params.workspace ?? "") : undefined,
	);
	const active_project = $derived.by(() => {
		if (active_thread !== undefined) return active_thread.primary_project;
		if (route_workspace_id === undefined) return undefined;
		return projects.find((candidate) => candidate.project_id === route_workspace_id);
	});
	const active_workspace_id = $derived(active_project?.project_id);
	/**
	 * Both `/t/` surfaces title themselves with their workspace: the conversation
	 * adds its own name after the project, and the workspace route is the project
	 * alone, which is exactly what it is. The root names nothing here — its
	 * sentence already says which project, and saying it twice on one screen
	 * would make the header look like the authority when the sentence is.
	 */
	const header_project = $derived(active_project);
	const thread_checklist = yield* ThreadChecklist;
	let checklist_state = $state.raw<ThreadChecklistState>(yield* thread_checklist.Current);
	const ApplyChecklistState = (next: ThreadChecklistState) =>
		Effect.gen(function* () {
			checklist_state = next;
		});
	yield* thread_checklist.Changes.pipe(
		Stream.runForEach(ApplyChecklistState),
		Effect.forkScoped,
	);
	const active_checklist = $derived(
		checklist_state._tag === "Ready" &&
			checklist_state.thread_id === active_thread?.thread_id &&
			conversation_plan_has_open_entries(checklist_state.plan)
			? checklist_state.plan
			: undefined,
	);
	const thread_orchestration = yield* ThreadOrchestrationRoster;
	let orchestration_state = $state.raw<ThreadOrchestrationState>(
		yield* thread_orchestration.Current,
	);
	const ApplyOrchestrationState = (next: ThreadOrchestrationState) =>
		Effect.gen(function* () {
			orchestration_state = next;
		});
	yield* thread_orchestration.Changes.pipe(
		Stream.runForEach(ApplyOrchestrationState),
		Effect.forkScoped,
	);
	const orchestration_lease = yield* thread_orchestration.Acquire(undefined);
	yield* Effect.addFinalizer(orchestration_lease.Release);
	/** The shell is persistent across route changes, so its one lease follows the active thread. */
	yield* orchestration_lease.Select(active_thread?.thread_id);
	const active_agents = $derived(
		orchestration_state._tag === "Ready" &&
			orchestration_state.thread_id === active_thread?.thread_id &&
			orchestration_state.entries.length > 0
			? orchestration_state.entries
			: undefined,
	);
	const thread_inspector_open = $derived(surface === "threads");
	/** Whichever workspace surface owns the column, when one is on screen at all. */
	const inspector_open = $derived(surface === "editor" || thread_inspector_open);

	/**
	 * Read once for the whole shell: every glass surface reads the store while
	 * rendering, so the stored preference has to reach it here rather than when
	 * the settings screen happens to be opened.
	 */
	const appearance = yield* AppearancePreferences;
	const browser_typography = yield* BrowserTypography;
	const initial_appearance = yield* appearance.Current;
	let stored_appearance = $state.raw<AppearanceState>(initial_appearance);
	const ApplyAppearance = (next: AppearanceState) =>
		Effect.gen(function* () {
			const next_typography = resolve_typography_preferences(next);
			stored_appearance = next;
			shader_enabled.set(next.shader_enabled);
			prose_width.set(next.prose_width ?? "balanced");
			typography.set(next_typography);
			yield* browser_typography.Apply(next_typography).pipe(Effect.ignore);
		});
	shader_enabled.set(initial_appearance.shader_enabled);
	prose_width.set(initial_appearance.prose_width ?? "balanced");
	typography.set(resolve_typography_preferences(initial_appearance));
	/**
	 * The store moves first so the rail answers in the same frame the control was
	 * pressed; the durable write follows and is never what the reader waits on.
	 * Written from the shell because the shell is what hands the rail its mode.
	 */
	yield* appearance.Changes.pipe(
		Stream.runForEach(ApplyAppearance),
		Effect.forkScoped,
	);
	yield* appearance.Load.pipe(Effect.forkScoped);

	const ApplyWorkspaceCatalogState = (next: WorkspaceCatalogState) =>
		Effect.gen(function* () {
			catalog_state = next;
		});

	/**
	 * The persistent shell is the sole catalog owner. Routes read this retained
	 * state synchronously and subscribe to Changes; they never open their own
	 * project/thread queries or authoritative streams.
	 */
	yield* workspace_catalog.Changes.pipe(
		Stream.runForEach(ApplyWorkspaceCatalogState),
		Effect.forkScoped,
	);
	yield* Effect.all(
		[workspace_catalog.SubscribeProjects, workspace_catalog.SubscribeThreadList],
		{ concurrency: "unbounded", discard: true },
	).pipe(Effect.forkScoped);

	const HydrateForge = (generation: number): Effect.Effect<void> =>
		Effect.gen(function* () {
			/**
			 * The connection is ready before this runs. Catalog subscriptions own
			 * their initial snapshots and recovery reads, while defaults hydrate in
			 * the persistent shell without delaying its first usable frame.
			 */
			yield* session_defaults.Refresh.pipe(Effect.ignore, Effect.forkScoped);
			const state = yield* client.ConnectionState;
			if (state.phase === "ready") {
				if (!IsCurrentForgeHydration(forge_gate, generation)) return;
				catalog_state = yield* workspace_catalog.Current;
				forge_gate = CompleteForgeHydration(forge_gate, generation);
				return;
			}
			yield* ApplyConnectionState(state);
		});

	/**
	 * A parked supervisor waits on a manual retry gate. Nobody is guaranteed to
	 * be looking at the overlay — the machine may simply have slept through the
	 * outage — so the shell re-arms the retry itself: on a slow cadence while
	 * the phase stays exhausted, and immediately when the window regains focus.
	 * Disconnection stays a state the app leaves on its own once Forge returns.
	 */
	let exhausted_retry_generation = 0;
	const RetryExhaustedConnectionLater = (generation: number) =>
		Effect.gen(function* () {
			yield* Effect.sleep("15 seconds");
			if (generation !== exhausted_retry_generation) return;
			if (forge_gate.state.phase !== "exhausted") return;
			yield* client.RetryConnection;
		});
	const RetryExhaustedConnectionNow = Effect.gen(function* () {
		if (forge_gate.state.phase !== "exhausted") return;
		yield* client.RetryConnection;
	});

	const ApplyConnectionState = (
		state: ArtisanConnectionState,
	): Effect.Effect<void> =>
		Effect.gen(function* () {
			forge_gate = ObserveForgeConnection(forge_gate, state);
			if (forge_gate.state.phase === "exhausted") {
				exhausted_retry_generation += 1;
				yield* RetryExhaustedConnectionLater(exhausted_retry_generation).pipe(
					Effect.forkScoped,
				);
			}
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
	 * Host notifications belong to the shell, not to the thread route: the
	 * entire point of one is the thread the reader is *not* looking at. Both
	 * surfaces run this same subscription — the desktop shell is a sandboxed
	 * page with no IPC of its own, so it reaches the operating system's
	 * notification centre exactly the way a paired browser tab does.
	 */
	const notifications = yield* SystemNotifications;
	const reader_attention = yield* BrowserReaderAttention;
	const NotifyForEvent = (envelope: EventEnvelope) =>
		Effect.gen(function* () {
			const thread = threads.find(
				(candidate) => candidate.thread_id === envelope.thread_id,
			);
			const focused = yield* reader_attention.Current;
			yield* notifications.Handle(envelope, {
				active_thread_id: active_thread?.thread_id,
				focused,
				/**
				 * A thread whose projection has not landed yet still deserves the
				 * notification; clicking it opens the workspace rather than nothing.
				 */
				route_path: thread === undefined ? "/" : ThreadRoutePathFor(thread),
				thread_title: thread?.title,
			});
		});

	/** Nothing to resynchronize: a notification missed while the socket was down is past. */
	const NoNotificationRecovery = Effect.gen(function* () {});

	yield* RunAuthoritativeSubscription(
		Effect.gen(function* () {
			return client.Events.pipe(Stream.filter(IsNotifiableEvent));
		}),
		NotifyForEvent,
		NoNotificationRecovery,
	).pipe(Effect.forkScoped);

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
			const reachable = yield* ProbeForgeRecoveryHealth(
				http_client.get(yield* ForgeHttpUrl("/health")),
			);
			/**
			 * An unreachable origin is not a Forge that is merely slow to return.
			 * The endpoint was handed to this document once and cannot change
			 * within it, so a Forge that died and came back on another port stays
			 * unreachable however long this loop runs. Only the desktop shell can
			 * ask `ae` for the new one, and the document title is the only channel
			 * it observes.
			 */
			forge_unreachable = !reachable;
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

	/**
	 * The first settled transcript otherwise pays Shiki's whole chunk graph at
	 * the moment it should be painting, and math or Mermaid pay theirs at first
	 * sight. Warming runs in idle slices after the shell is up, so by the time
	 * a thread opens the renderer modules are usually already cached.
	 */
	yield* WarmConversationRenderers.pipe(Effect.forkScoped);

	desktop_runtime = yield* RunBrowserDom(
		() => RuntimeSurfaceFor(globalThis.navigator?.userAgent ?? "") === "desktop",
	);
	/**
	 * macOS is the one desktop platform whose window controls overlay the
	 * strip's left end instead of its right, exactly where the left-anchored
	 * identity would sit. The identity swaps sides there rather than padding
	 * itself past the traffic lights: a right-aligned line reads cleanly and
	 * the lights keep their native position.
	 */
	mac_desktop = yield* RunBrowserDom(() =>
		IsMacDesktop(globalThis.navigator?.userAgent ?? ""),
	);
</script>

<svelte:window onfocus={yield* RetryExhaustedConnectionNow} />

<ModeWatcher defaultMode="dark" />
<DevInstanceBadge />
<AttentionTitleMarker {threads} {forge_unreachable} />

{#snippet primary()}
	{@render children()}
{/snippet}

{#snippet secondary()}
	<ThreadPanel
		plan={active_checklist}
		agents={active_agents}
		oninspectagent={thread_orchestration.Inspect}
		thread_id={active_thread?.thread_id}
		project_id={active_project?.project_id}
		project_root_path={active_project?.root_path}
		workspace_id={active_workspace_id}
	/>
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
					<!--
						The line fades into the window controls rather than running under
						them: the controls float over one end of this strip, and which end
						is the platform's business — trailing on Windows and Linux, leading
						on macOS, where the identity is right-aligned away from the lights.
					-->
					<div
						class={mac_desktop
							? "fade-edge-start flex w-full min-w-0 items-center justify-end pl-16 pr-6"
							: "fade-edge-end flex w-full min-w-0 items-center pr-6"}
					>
						{@render workspace_header()}
					</div>
				{/if}
			</div>
			{#if inspector_open}
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
					{threads_loaded}
					header={desktop_runtime || header_project === undefined
						? undefined
						: workspace_header}
					{thread_rail}
					thread_id={active_thread?.thread_id}
					workspace_id={active_workspace_id}
					secondary={surface === "editor" ? editor_files : secondary}
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

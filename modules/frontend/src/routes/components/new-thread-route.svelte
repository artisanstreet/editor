<script lang="ts" effect>
	/**
	 * A new thread, which is what the application opens on.
	 *
	 * There is one question here and it is the message. Above the composer, a
	 * panel holds what you would otherwise have to leave to find — where you
	 * have been, with a year of token spend beside it.
	 *
	 * The same surface serves `/` and `/t/<workspace>`. The difference is only
	 * where the project comes from: the root uses the preferred recent project,
	 * while the routed form takes it from the URL. Nothing durable exists until
	 * the first send, which creates the thread and hands over to its own route.
	 */
	import { Effect, Stream } from "effect";
	import type {
		Project,
		SurfaceUsageDailyBucket,
		ThreadSessionPolicy,
	} from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import VerticalCalendarActivityGrid, {
		type CalendarActivity,
	} from "$lib/components/activity/vertical-calendar-activity-grid.svelte";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";
	import type { ComposerActionFailure } from "$lib/composer/action-failure";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import { PreferredProject, RecentProjects } from "$lib/root/project-catalog";
	import { SeededDraftPolicy } from "$lib/root/draft-policy";
	import { DraftThreadController, type DraftThreadState } from "$lib/root/draft-thread";
	import {
		WorkspaceCatalogController,
		type WorkspaceCatalogState,
	} from "$lib/root/workspace-catalog-controller";
	import {
		RetryNewThreadDraft,
		SubmitNewThreadDraft,
		new_thread_draft_key,
	} from "$lib/root/new-thread-draft";
	import {
		FormatRecentThreadTime,
		SortRecentThreads,
		ThreadLastMessageAt,
		ThreadRoutePath,
		ThreadRoutePathFor,
	} from "$lib/root/thread-navigation";
	import { SessionDefaultsController } from "$lib/settings/session-defaults-controller";
	import { thread_display_title, thread_title_mode } from "$lib/threads/title";
	import ComposerActionFailureView from "./composer/action-failure.svelte";
	import DropdownHoverSurface from "./dropdown-hover-surface.svelte";
	import ThreadComposer from "./thread-composer.svelte";

	let {
		workspace_id,
	}: {
		/**
		 * The project named by the route, when one is. Absent on `/`, where the
		 * preferred recent project is used.
		 */
		readonly workspace_id?: string;
	} = $props();

	const navigation = yield* RouteNavigation;
	const draft_thread = yield* DraftThreadController;
	const session_defaults = yield* SessionDefaultsController;
	const workspace_catalog = yield* WorkspaceCatalogController;
	const draft_key = $derived(new_thread_draft_key(workspace_id));

	/** The shell owns hydration; a new route reads its retained snapshot immediately. */
	let catalog_state = $state.raw<WorkspaceCatalogState>(yield* workspace_catalog.Current);
	const ApplyCatalogState = (next: WorkspaceCatalogState) =>
		Effect.gen(function* () {
			catalog_state = next;
		});
	yield* workspace_catalog.Changes.pipe(
		Stream.runForEach(ApplyCatalogState),
		Effect.forkScoped,
	);
	const projects = $derived(catalog_state.projects);
	const threads = $derived(catalog_state.threads);
	const catalog_read = $derived(catalog_state.projects_loaded);

	const recents = $derived(RecentProjects(projects, threads));

	/**
	 * Where the reader last was, read once rather than followed: it seeds the
	 * project on arrival, and a draft edited behind this surface has no business
	 * changing it afterwards.
	 */
	const opening_state = yield* draft_thread.Current;
	const opening_project_id =
		opening_state._tag === "Uninitialized" ? undefined : opening_state.project?.project_id;
	/**
	 * A retained created draft needs its recovery action on arrival. A draft
	 * created by this mount does not: it is still following the ordinary route
	 * handoff until navigation actually fails.
	 */
	let show_draft_recovery = $state(opening_state._tag === "Created");
	/** What the last draft action refused, reported over the composer it refused. */
	let draft_failure = $state.raw<ComposerActionFailure | undefined>(undefined);
	const DismissDraftFailure = Effect.gen(function* () {
		draft_failure = undefined;
	});
	/**
	 * A URL that names a workspace is resolved against the catalog rather than
	 * trusted — a segment naming a project Forge does not have is not a
	 * workspace, and this surface must not compose into a fiction.
	 */
	const routed_project = $derived(
		workspace_id === undefined
			? undefined
			: recents.find((recent) => recent.project.project_id === workspace_id)?.project,
	);
	const project = $derived(
		workspace_id === undefined
			? PreferredProject(recents, opening_project_id)
			: routed_project,
	);
	const detached = $derived(
		workspace_id !== undefined && catalog_read && routed_project === undefined,
	);

	let draft_state = $state.raw<DraftThreadState>(opening_state);
	let draft_revision = $state(yield* draft_thread.CurrentRevision);
	const ApplyDraftState = (next: DraftThreadState) =>
		Effect.gen(function* () {
			draft_state = next;
		});
	const ApplyDraftRevision = (next: number) =>
		Effect.gen(function* () {
			draft_revision = next;
		});
	yield* draft_thread.Changes.pipe(Stream.runForEach(ApplyDraftState), Effect.forkScoped);
	yield* draft_thread.RevisionChanges.pipe(
		Stream.runForEach(ApplyDraftRevision),
		Effect.forkScoped,
	);
	/** A first message already in flight owns the project it was created with. */
	const locked = $derived(draft_state._tag === "Created");

	/**
	 * Defaults are hydrated by the persistent shell and retained in the app-scoped
	 * controller, so a new route can open with the same model and permission
	 * without paying another Forge round trip. Before that hydration completes,
	 * the controller's offline snapshot is the honest fallback.
	 */
	const LoadSeedPolicy = Effect.gen(function* () {
		const snapshot = yield* session_defaults.Current;
		return SeededDraftPolicy(snapshot.catalog, snapshot.defaults);
	});

	/**
	 * Pointing the draft at the project this route currently resolves.
	 *
	 * Read from the controller rather than from the reactive mirror above: the
	 * mirror is written by this very block, and a dependency on it would make
	 * aligning the draft re-align it forever. The seed policy is only fetched
	 * when the draft has none, so catalog refreshes cost nothing.
	 */
	/**
	 * Whether the controller is actually holding the project this surface offers
	 * to send into.
	 *
	 * The surface resolving a project is not the same fact as the draft carrying
	 * one: the first is read from the catalog here, the second is written into
	 * the controller by the alignment below, and only the second is what a send
	 * is checked against. Reading the refusal is what keeps them from disagreeing
	 * — a discarded `false` armed the composer over a draft that would reject it,
	 * and the rejection had nowhere to surface.
	 */
	let draft_aligned = $state(false);

	const AlignDraft = (target: Project | undefined, _mirrored_revision: number) =>
		Effect.gen(function* () {
			if (target === undefined) {
				draft_aligned = false;
				return;
			}
			const current = yield* draft_thread.Current;
			/** A retained submission already owns its project; there is nothing to align. */
			if (current._tag === "Created") {
				draft_aligned = true;
				return;
			}
			/**
			 * The mirrored revision says *when* to align, not what to align against.
			 * It arrives through a forked stream, so it trails the controller by a
			 * scheduler turn, and passing it through refused every alignment in the
			 * gap between a reset and the mirror catching up — leaving the draft
			 * with no project while the surface still resolved one.
			 *
			 * Read here, before the seed policy below, the revision is this
			 * attempt's own starting point. A reset landing while that policy loads
			 * still moves it, and the controller still rejects the write, which is
			 * the stale-alignment race the guard is there for.
			 */
			const expected_revision = yield* draft_thread.CurrentRevision;
			const policy =
				current._tag === "Ready" && current.policy !== undefined
					? current.policy
					: yield* LoadSeedPolicy;
			/** The controller checks and writes under the same lock used by an explicit reset. */
			draft_aligned = yield* draft_thread.AlignAtRevision(expected_revision, target, policy);
		});
	/** The mirrored revision is read purely so a reset re-runs the alignment. */
	yield* AlignDraft(project, draft_revision);
	/** The one fact a send is actually gated on: a project, held by the draft. */
	const draft_ready = $derived(project !== undefined && draft_aligned);

	/**
	 * The created draft is already retained before this navigation begins. Keep
	 * its recovery action hidden during the normal handoff and reveal it only if
	 * the route cannot be opened.
	 */
	const NavigateCreatedDraft = (path: string) =>
		Effect.gen(function* () {
			show_draft_recovery = false;
			yield* navigation.Navigate(path).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
						show_draft_recovery = true;
					}),
				),
			);
		});

	/**
	 * The durable thread materializes only here, at the first send: it is created
	 * with the selected project, receives the draft's session policy, and the
	 * submission is handed to the routed thread, which owns the durable message
	 * pipeline.
	 */
	const SubmitFirstMessage = (submission: ComposerSubmission) =>
		Effect.gen(function* () {
			const created = yield* SubmitNewThreadDraft(draft_key, submission);
			yield* NavigateCreatedDraft(
				ThreadRoutePath(created.project.project_id, created.thread_id),
			);
		});

	/**
	 * The picker persists a policy and waits to be told what was actually applied,
	 * then remembers that as the session default. A draft has no server to answer
	 * with authority, so what it stored is the answer.
	 */
	const UpdateDraftPolicy = (policy: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			yield* draft_thread.UpdatePolicy(policy);
			return policy;
		});

	const RetryDraftNavigation = Effect.gen(function* () {
		const created = yield* RetryNewThreadDraft(draft_key);
		yield* NavigateCreatedDraft(
			ThreadRoutePath(created.project.project_id, created.thread_id),
		);
	});

	/**
	 * Every thread, freshest first — the same order and the same relative stamps
	 * the rail's list carries, because this is the same question asked from the
	 * opening surface rather than a second history with its own idea of recent.
	 * Unlike the rail it does not drop working threads into a group of their
	 * own: there is no run to watch from here, only somewhere to return to.
	 */
	const recent_threads = $derived(SortRecentThreads(threads));
	/** Captured on mount so the relative stamps never sit stale on screen. */
	const now_ms = Date.now();

	/**
	 * A year of daily token spend, in the calendar the root surface has carried
	 * since the beginning. It answers a question the thread list cannot — not
	 * where you were, but how much you have been spending and on what — which
	 * is why it sits beside the list rather than under it.
	 */
	const usage_day_count = 365;
	const day_in_ms = 86_400_000;
	const client = yield* ArtisanClient;
	let usage = $state.raw<ReadonlyArray<SurfaceUsageDailyBucket>>([]);
	/**
	 * Before any usage exists the grid still draws its full year of zero-token
	 * days. An empty canvas would collapse the pane's shape on a fresh install
	 * and read as a fault rather than as nothing having happened yet.
	 */
	const empty_usage_days = (): ReadonlyArray<CalendarActivity> =>
		Array.from({ length: usage_day_count }, (_, index) => ({
			date: new Date(now_ms - (usage_day_count - 1 - index) * day_in_ms)
				.toISOString()
				.slice(0, 10),
			engines: [],
			tokens: 0,
		}));
	const activities = $derived(
		usage.length === 0
			? empty_usage_days()
			: usage.map(
					(bucket): CalendarActivity => ({
						date: bucket.date,
						engines: bucket.engines.map((slice) => ({
							...(slice.engine_id === undefined ? {} : { engine_id: slice.engine_id }),
							...(slice.model_id === undefined ? {} : { model_id: slice.model_id }),
							tokens: slice.input_tokens + slice.output_tokens,
						})),
						tokens: bucket.input_tokens + bucket.output_tokens,
					}),
				),
	);
	/**
	 * Usage is decoration on a surface whose one job is to start a thread, so a
	 * Forge that cannot answer leaves the year of zeroes standing rather than
	 * raising a banner over a composer that still works perfectly well.
	 */
	const LoadUsage = Effect.gen(function* () {
		const snapshot = yield* client.GetSurfaceUsageDaily({ day_count: usage_day_count });
		usage = snapshot.buckets;
	}).pipe(Effect.catch(() => Effect.void));
	yield* LoadUsage;
</script>

<svelte:head>
	<title>{project === undefined ? "Artisan Editor" : `${project.display_name} › Artisan Editor`}</title>
</svelte:head>

<main class="relative h-full min-h-0 overflow-hidden" aria-label="New thread">
	<!--
		This surface belongs to the composer, not to the viewport: it takes the
		same prose-column placement and width bound as the composer's card, and
		follows it when the rail shifts that column off the window's centre, so
		the two read as one unit. Centring in the raw frame instead put them on
		different axes, and they visibly disagreed.
	-->
	<!--
		The composer is a floating card in its own frame, so this frame's full
		height is not the space actually free to centre in. Reserving the card's
		footprint at the bottom is what makes "centred" mean centred in the gap
		above it rather than in a viewport the composer covers the foot of.
	-->
	<div class="prose-column-frame absolute inset-0 flex flex-col items-center justify-center pb-44">
		<div class="prose-column flex w-full max-w-(--prose-width) flex-col gap-3 text-center">
			{#if detached}
				<p class="text-lg text-foreground">This project is not attached.</p>
				<p class="max-w-sm text-sm text-muted-foreground">
					Forge owns the project catalog and has no project by this identity.
				</p>
				<a
					class="text-sm text-(--banner-info) underline-offset-2 hover:underline"
					href="/"
				>
					Start a thread somewhere else
				</a>
			{:else if !catalog_read}
				<p class="text-sm text-muted-foreground" role="status">Loading projects…</p>
			{:else}
				<!--
					The redesigned opening surface: a panel on the composer's column,
					its panes split 2/3 · 1/3. Nothing is drawn around or between them —
					the two are told apart by their own contents and the space between,
					which is all the separation a list and a chart need.

					Every track here is spelled `minmax(0, …)` for one reason: a bare
					`fr` is `minmax(auto, 1fr)`, so a track never shrinks below its
					content and the ratio is only a wish. Both axes were losing that
					argument. A long enough list stretched the panel past the composer
					and off the viewport instead of scrolling, and the canvas — 300px
					wide by default until it has measured its host — pushed the right
					column out until the split read as half and half rather than 2 : 1.

					`min-h-0` retracts the same automatic minimum on the panel itself,
					which it has as a flex item. Only past all of them is a pane free to
					be smaller than its content, which is the permission its own
					overflow (or its canvas) needs before either can do anything.
				-->
				<div class="grid aspect-3/2 w-4/5 min-h-0 grid-cols-[minmax(0,2fr)_minmax(0,1fr)] grid-rows-[minmax(0,1fr)] self-center">
					<!-- Where you have been, in a list that scrolls inside its pane. -->
					<div class="flex min-h-0 flex-col text-left">
						<div class="docs-scroll-fade relative min-h-0 flex-1 overflow-x-hidden overflow-y-auto p-1">
							{#if recent_threads.length === 0}
								<p class="px-2 py-2 text-sm text-muted-foreground">No threads yet.</p>
							{:else}
								<!-- The rail's hover exactly: one pill slides between rows. -->
								<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-lg)]">
									{#snippet children({ move_hover })}
										<div class="flex flex-col">
											{#each recent_threads as thread (thread.thread_id)}
												{@const thread_mark = EngineMarkFor(thread.engine_id)}
												{@const ThreadMark = thread_mark.icon}
												<a
													href={ThreadRoutePathFor(thread)}
													class="relative flex min-w-0 items-center gap-2 rounded-lg px-2 py-2 text-sm text-muted-foreground outline-none transition-colors duration-(--duration-fast) ease-in-out hover:text-foreground-extra focus-visible:text-foreground-extra motion-reduce:transition-none"
													onpointerenter={move_hover}
													onpointermove={move_hover}
													onfocusin={move_hover}
												>
													<!-- The list names the same thing the rail does: what the thread runs on. -->
													<ThreadMark class={EngineMarkClass(thread_mark, "size-4 shrink-0")} />
													<span class="min-w-0 flex-1 truncate"
														>{thread_display_title(thread, $thread_title_mode)}</span
													>
											<span class="shrink-0 whitespace-nowrap text-xs text-muted-foreground">
												{FormatRecentThreadTime(ThreadLastMessageAt(thread), now_ms)}
											</span>
												</a>
											{/each}
										</div>
									{/snippet}
								</DropdownHoverSurface>
							{/if}
						</div>
					</div>

					<!--
						The grid measures its own host and redraws on resize, so the pane
						only has to be a box with a real height: `min-h-0` and `flex` are
						what give it one inside an aspect-bound row, and the canvas fills
						whatever that turns out to be.
					-->
					<div class="flex min-h-0 flex-col p-2" aria-label="Token usage">
						<VerticalCalendarActivityGrid {activities} />
					</div>
				</div>
			{/if}
		</div>
	</div>

	{#if locked && show_draft_recovery}
		<div
			class="fixed inset-x-0 bottom-6 z-30 mx-auto flex w-fit items-center gap-3 rounded-lg border border-border bg-background px-4 py-3 shadow-lg"
			role="status"
		>
			<span>Your new thread is ready. Its first message is retained until delivery succeeds.</span>
			<button type="button" onclick={yield* RetryDraftNavigation}>Open thread and retry</button>
		</div>
	{/if}

	<ComposerActionFailureView failure={draft_failure} ondismiss={DismissDraftFailure} />

	<!-- The composer pins to the page bottom; its first send creates the thread and routes into it. -->
	<!--
		The draft is keyed to the surface. A routed workspace keeps its own draft;
		the root keeps one stable draft while its preferred project is resolved.
	-->
	{#key draft_revision}
		<ThreadComposer
			disabled={!draft_ready || locked}
			{draft_key}
			onpolicychange={UpdateDraftPolicy}
			onsubmit={SubmitFirstMessage}
			policy={draft_state._tag === "Uninitialized" ? undefined : draft_state.policy}
		/>
	{/key}
</main>

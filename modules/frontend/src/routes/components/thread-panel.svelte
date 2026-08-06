<script lang="ts" effect>
	import { page } from "$app/state";
	import { Clock, Effect, Fiber, Option, Schedule, Stream } from "effect";
	import type {
		Project,
		ProjectDiff,
		RepositoryDiffSnapshot,
		ProjectRepository,
		ThreadListItem,
	} from "@artisan/protocol";
	import { ArtisanClient, type ThreadListUpdate } from "@artisan/transport/client";
	import { BannerService } from "$lib/banner/service";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import {
		DraftThreadController,
		type DraftThreadState,
	} from "$lib/root/draft-thread";
	import {
		HasReportableWork,
	} from "$lib/vcs/diff-presentation";
	import {
		ApplyRootThreadListUpdate,
		FormatRecentThreadTime,
		ResolveThreadRoute,
	} from "$lib/root/thread-navigation";
	import ProjectFolderPicker from "./project-folder-picker.svelte";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";
	import ProjectSelector from "./panel/project-selector.svelte";
	import ShaderSettings from "./panel/shader-settings.svelte";

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	const draft_thread = yield* DraftThreadController;
	let threads = $state.raw<ReadonlyArray<ThreadListItem>>([]);
	let existing_projects = $state.raw<ReadonlyArray<Project>>([]);
	let repositories = $state.raw<ReadonlyMap<string, ProjectRepository>>(new Map());
	let picker_open = $state(false);
	let project_diff = $state.raw<RepositoryDiffSnapshot | undefined>(undefined);
	/** Stamped when a reading lands: the detail renders the commit time as an interval. */
	let read_at_ms = $state(0);
	/** True once the current project's diff request has run to completion, hit or miss. */
	let diff_settled = $state(false);
	/**
	 * The lip's open state is decided by an RPC that lands just after arrival, and
	 * the panel is one instance across navigations, so without a gate the reveal
	 * would play on every thread visited. The gate disarms on each project change
	 * and re-arms only after that project's settled state has painted; changes
	 * while the reader is on a thread animate normally.
	 */
	let lip_animate = $state(false);
	let diff_detail_open = $state(false);
	let diff_close_fiber: Fiber.Fiber<void> | undefined;
	let assigning = $state(false);
	let draft_state = $state.raw<DraftThreadState>(yield* draft_thread.Current);
	const ApplyDraftState = (next: DraftThreadState) =>
		Effect.gen(function* () {
			draft_state = next;
		});
	yield* draft_thread.Changes.pipe(
		Stream.runForEach(ApplyDraftState),
		Effect.forkScoped,
	);

	const route_id = $derived(page.params.thread);
	/**
	 * The panel mounts on the draft route and on concrete threads alike, and
	 * only the latter carries a route id. A draft has no durable thread yet, so
	 * its project lives in the draft store until the first send creates one.
	 */
	const is_draft = $derived(route_id === undefined);
	const thread = $derived(
		route_id === undefined
			? undefined
			: Option.getOrUndefined(ResolveThreadRoute(threads, route_id)),
	);
	const project = $derived(
		is_draft && draft_state._tag !== "Uninitialized"
			? draft_state.project
			: thread?.primary_project,
	);
	/**
	 * The identifier alone, so effects keyed on it hold still while the project
	 * object is refreshed by ordinary thread activity.
	 */
	const pinned_project_id = $derived(project?.project_id);
	/**
	 * Resolved here rather than inside the snippet that renders it. Read from a
	 * snippet's `{@const}`, the lookup did not re-run when the map arrived, so the
	 * trigger kept showing the fallback path until some unrelated re-render — a
	 * hover — recomputed it.
	 */
	const active_repository = $derived(
		project === undefined ? undefined : repositories.get(project.project_id),
	);
	/**
	 * One derived value gates the lip's open state and its content alike, so the
	 * animation and what it reveals cannot disagree. Every row the detail can
	 * render counts as work, including the ones that carry no line counts.
	 */
	const reportable = $derived(
		project_diff !== undefined && HasReportableWork(project_diff) ? project_diff : undefined,
	);

	/**
	 * Hover opens the detail, but the surface is portalled, so the pointer leaves
	 * the marker on its way there. A short grace period spans that gap; entering
	 * either the marker or the surface cancels it.
	 */
	const KeepDiffDetailOpen = Effect.gen(function* () {
		if (diff_close_fiber !== undefined) yield* Fiber.interrupt(diff_close_fiber);
		diff_detail_open = true;
	});

	const CloseDiffDetailSoon = Effect.gen(function* () {
		if (diff_close_fiber !== undefined) yield* Fiber.interrupt(diff_close_fiber);
		const CloseAfterDelay = Effect.gen(function* () {
			yield* Effect.sleep("120 millis");
			diff_detail_open = false;
		});
		diff_close_fiber = yield* CloseAfterDelay.pipe(Effect.forkScoped);
	});
	const ApplyThreadListUpdate = (update: ThreadListUpdate) =>
		Effect.gen(function* () {
			threads = ApplyRootThreadListUpdate(threads, update);
		});

	const RefreshThreads = Effect.gen(function* () {
		const next_threads = yield* client.ListThreads;
		yield* ApplyThreadListUpdate({
			journal_sequence: 0,
			threads: next_threads,
			type: "snapshot",
		});
	});

	const OpenProjectPicker = () =>
		Effect.gen(function* () {
			assigning = true;
			const catalog = yield* client.ListProjects;
			existing_projects = catalog.projects;
			picker_open = true;
		}).pipe(
			Effect.ensuring(
				Effect.gen(function* () {
					assigning = false;
				}),
			),
			Effect.catch((error) =>
				Effect.gen(function* () {
					yield* banner.error("Could not load projects", { description: error.message });
				}),
			),
		);

	const AssignProject = (candidate: Project) =>
		Effect.gen(function* () {
			/** A draft has no durable thread; the choice lives client-side until first send. */
			if (is_draft) {
				yield* draft_thread.SelectProject(candidate);
				picker_open = false;
				return;
			}
			const thread_id = thread?.thread_id;
			if (thread_id === undefined) return;
			assigning = true;
			yield* client.Command({
				payload: { project_id: candidate.project_id, type: "thread.project.assign" },
				thread_id,
			});
			picker_open = false;
			yield* RefreshThreads;
		}).pipe(
			Effect.ensuring(
				Effect.gen(function* () {
					assigning = false;
				}),
			),
			Effect.catch((error) =>
				Effect.gen(function* () {
					yield* banner.error("Could not assign project", { description: error.message });
				}),
			),
		);

	/** Sentinel for the item that leads to the folder picker; never a real project id. */
	const BROWSE_VALUE = "artisan:browse-for-folder";

	const RequestProject = (value: string) =>
		Effect.gen(function* () {
			if (value === BROWSE_VALUE) {
				yield* OpenProjectPicker();
				return;
			}
			const candidate = existing_projects.find(
				(project_candidate) => project_candidate.project_id === value,
			);
			if (candidate !== undefined) yield* AssignProject(candidate);
		});

	const RequestProjectList = (is_open: boolean) =>
		Effect.gen(function* () {
			if (is_open) yield* LoadProjects;
		});

	/**
	 * The panel mounts before the transport is necessarily ready, and both reads
	 * below fail outright on a cold start. Swallowing that failure left an empty
	 * repository map for the rest of the session — the header showed a project's
	 * location instead of its branch until something re-opened the select and
	 * happened to read it again.
	 */
	const ColdStartRetrySchedule = Schedule.exponential("100 millis").pipe(
		Schedule.upTo({ duration: "5 seconds" }),
	);

	const LoadProjects = Effect.gen(function* () {
		const catalog = yield* client.ListProjects.pipe(
			Effect.retry({ schedule: ColdStartRetrySchedule }),
		);
		existing_projects = catalog.projects;

		/**
		 * Repository state is read separately so a slow or missing checkout never
		 * delays the project names themselves.
		 */
		const result = yield* client.GetProjectRepositories().pipe(
			Effect.retry({ schedule: ColdStartRetrySchedule }),
			Effect.catch(() =>
				Effect.gen(function* () {
					return { repositories: [] };
				}),
			),
		);
		repositories = new Map(
			result.repositories.map((entry) => [entry.project_id, entry.repository]),
		);
	}).pipe(
		Effect.catch((error) =>
			Effect.gen(function* () {
				yield* banner.error("Could not load projects", { description: error.message });
			}),
		),
	);

	const NextAnimationFrame = Effect.gen(function* () {
		yield* Effect.callback<void>((resume) => {
			const frame = requestAnimationFrame(() =>
				resume(
					Effect.gen(function* () {
					}),
				),
			);
			return Effect.gen(function* () {
				cancelAnimationFrame(frame);
			});
		});
	});

	const ApplyDiff = (requested: string | undefined, snapshot: ProjectDiff | undefined) =>
		Effect.gen(function* () {
			const now_ms = yield* Clock.currentTimeMillis;
			/** A reply for a project we have since left must not label the new one. */
			if (requested !== project?.project_id) return;
			project_diff = snapshot?.state === "repository" ? snapshot : undefined;
			read_at_ms = now_ms;
		});

	/**
	 * Reads one project rather than the catalog: each costs a status walk on the
	 * Forge machine, and only the pinned project is ever shown.
	 */
	const LoadDiff = (requested: string) =>
		Effect.gen(function* () {
			const result = yield* client.GetProjectDiffs([requested]);
			const snapshot = result.diffs.find(
				(entry) => entry.project_id === requested,
			)?.diff;
			yield* ApplyDiff(requested, snapshot);
		}).pipe(
			/** A diff is a nicety; failing to read one must not raise a banner over the thread. */
			Effect.catch(() =>
				Effect.gen(function* () {
					yield* ApplyDiff(requested, undefined);
				}),
			),
		);

	const RefreshDiff = (requested: string | undefined) =>
		Effect.gen(function* () {
			diff_settled = false;
			lip_animate = false;
			yield* ApplyDiff(requested, undefined);
			if (requested !== undefined) yield* LoadDiff(requested);
			/** A settle for a project since left must not arm the next one's reveal. */
			if (requested === project?.project_id) diff_settled = true;
		});


	/**
	 * Read at mount, not on first open. The header shows the pinned project's
	 * branch and remote, so waiting for the select to be opened left it showing a
	 * bare filesystem path — and fetching on open also reflows the menu under the
	 * pointer as the rows splice in.
	 */
	yield* Effect.forkScoped(LoadProjects);

	/**
	 * Keyed on the identifier, not the project object: SER interrupts the stale
	 * read when the workspace changes and owns the replacement read's lifecycle.
	 */
	yield* RefreshDiff(pinned_project_id);

	/**
	 * Two frames, not one: the settled open state and the arming write could
	 * otherwise land in the same style recalc, which would play the reveal the
	 * gate exists to suppress. The first frame paints the state, the second arms.
	 */
	const ArmLip = (is_settled: boolean, is_animated: boolean) =>
		Effect.gen(function* () {
			if (!is_settled || is_animated) return;
			yield* NextAnimationFrame;
			yield* NextAnimationFrame;
			if (diff_settled && !lip_animate) lip_animate = true;
		});
	yield* ArmLip(diff_settled, lip_animate);

	yield* RefreshThreads;

	yield* RunAuthoritativeSubscription(
		client.SubscribeThreadList,
		ApplyThreadListUpdate,
		RefreshThreads,
	).pipe(Effect.forkScoped);
</script>

<ProjectFolderPicker bind:open={picker_open} onselect={AssignProject} />

<div class="relative flex h-full min-h-0 flex-col p-4">
	<!--
		The workspace header already names a pinned thread's repository and
		branch, so repeating it here would say the same thing twice. The card
		stays only where it still does work: picking a project for a draft, or
		pinning a thread that has none yet.
	-->
	{#if is_draft || project === undefined}
		<ProjectSelector
			active_repository={active_repository}
			assigning={assigning}
			bind:diff_detail_open
			disabled={assigning || (!is_draft && thread === undefined) || draft_state._tag === "Created"}
			existing_projects={existing_projects}
			lip_animate={lip_animate}
			onclose={CloseDiffDetailSoon}
			keep_open={KeepDiffDetailOpen}
			onopenchange={RequestProjectList}
			onvaluechange={RequestProject}
			{project}
			read_at_ms={read_at_ms}
			{reportable}
			{repositories}
		/>
	{/if}
	<ShaderSettings />
</div>

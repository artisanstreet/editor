<script lang="ts" effect>
	import { page } from "$app/state";
	import ChevronLeft from "@tabler/icons-svelte/icons/chevron-left";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import Folder from "@tabler/icons-svelte/icons/folder";
	import GitBranch from "@tabler/icons-svelte/icons/git-branch";
	import HelpCircle from "@tabler/icons-svelte/icons/help-circle";
	import Settings from "@tabler/icons-svelte/icons/settings";
	import { Clock, Effect, Fiber, Option, Queue, Schedule } from "effect";
	import type {
		Project,
		ProjectDiff,
		RepositoryDiffSnapshot,
		ProjectDirectoryId,
		ProjectRepository,
		ProjectDirectoryList,
		ThreadListItem,
	} from "@artisan/protocol";
	import { ArtisanClient, type ThreadListUpdate } from "@artisan/transport/client";
	import { BannerService } from "$lib/banner/service";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import { Button } from "$lib/components/ui/button";
	import * as Dialog from "$lib/components/ui/dialog";
	import { LipCard } from "$lib/components/ui/lip-card";
	import { Popover, PopoverContent, PopoverTrigger } from "$lib/components/ui/popover";
	import * as ScrollArea from "$lib/components/ui/scroll-area";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
	} from "$lib/components/ui/select";
	import { MakeFollowHighlight } from "$lib/components/dropdown-highlight";
	import { draft_thread_project } from "$lib/root/draft-thread";
	import { ShortProjectPath } from "$lib/root/project-path";
	import {
		ComparisonLabel,
		DiffCount,
		DiffFileCount,
		HasReportableWork,
	} from "$lib/vcs/diff-presentation";
	import {
		RepositoryLinkLabel,
		RepositoryMarkClass,
		RepositoryMarkFor,
	} from "$lib/vcs/presentation";
	import {
		ApplyRootThreadListUpdate,
		FormatRecentThreadTime,
		ResolveThreadRoute,
	} from "$lib/root/thread-navigation";
	import DiffCountsRow from "./diff-counts-row.sv";
	import DropdownHoverSurface from "./dropdown-hover-surface.sv";
	import ShaderDevPanel from "./shader-dev-panel.sv";
	import ShaderGlassSurface from "./shader-glass-surface.sv";

	type PanelState = "closed" | "open" | "closing";

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	const FollowHighlight = yield* MakeFollowHighlight;
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
	let project_directories = $state.raw<ProjectDirectoryList | undefined>();
	let project_directory_history = $state.raw<ReadonlyArray<ProjectDirectoryId>>([]);
	let panel_state: PanelState = $state("closed");
	let close_fiber: Fiber.Fiber<void> | undefined;

	const route_id = $derived(page.params.id);
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
	const project = $derived(is_draft ? $draft_thread_project : thread?.primary_project);
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
		diff_close_fiber = yield* Effect.sleep("120 millis").pipe(
			Effect.andThen(
				Effect.sync(() => {
					diff_detail_open = false;
				}),
			),
			Effect.forkScoped,
		);
	});
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

	const OpenProjectPicker = () =>
		Effect.gen(function* () {
			yield* Effect.sync(() => {
				assigning = true;
			});
			const catalog = yield* client.ListProjects;
			const listing = yield* client.ListProjectDirectories();
			yield* Effect.sync(() => {
				existing_projects = catalog.projects;
				project_directories = listing;
				project_directory_history = [];
				picker_open = true;
			});
		}).pipe(
			Effect.ensuring(
				Effect.sync(() => {
					assigning = false;
				}),
			),
			Effect.catch((error) =>
				banner.error("Could not load projects", { description: error.message }),
			),
		);

	const LoadProjectDirectories = (
		parent_directory_id?: ProjectDirectoryId,
		history: ReadonlyArray<ProjectDirectoryId> = [],
	) =>
		Effect.gen(function* () {
			yield* Effect.sync(() => {
				assigning = true;
			});
			const listing = yield* client.ListProjectDirectories(
				parent_directory_id === undefined ? undefined : { parent_directory_id },
			);
			yield* Effect.sync(() => {
				project_directories = listing;
				project_directory_history = history;
			});
		}).pipe(
			Effect.ensuring(
				Effect.sync(() => {
					assigning = false;
				}),
			),
			Effect.catch((error) =>
				banner.error("Could not load project folders", { description: error.message }),
			),
		);

	const BrowseProjectDirectory = (directory_id: ProjectDirectoryId) =>
		LoadProjectDirectories(directory_id, [...project_directory_history, directory_id]);

	const BrowseParentProjectDirectory = () => {
		const history = project_directory_history.slice(0, -1);
		return LoadProjectDirectories(history.at(-1), history);
	};

	const AssignProject = (candidate: Project) =>
		Effect.gen(function* () {
			/** A draft has no durable thread; the choice lives client-side until first send. */
			if (is_draft) {
				yield* Effect.sync(() => {
					draft_thread_project.set(candidate);
					picker_open = false;
				});
				return;
			}
			const thread_id = thread?.thread_id;
			if (thread_id === undefined) return;
			yield* Effect.sync(() => {
				assigning = true;
			});
			yield* client.Command({
				payload: { project_id: candidate.project_id, type: "thread.project.assign" },
				thread_id,
			});
			yield* Effect.sync(() => {
				picker_open = false;
			});
			yield* RefreshThreads;
		}).pipe(
			Effect.ensuring(
				Effect.sync(() => {
					assigning = false;
				}),
			),
			Effect.catch((error) =>
				banner.error("Could not assign project", { description: error.message }),
			),
		);

	/**
	 * The select fires from a plain callback, so requests cross into Effect through
	 * a queue rather than being forked ad hoc at the call site.
	 */
	type ProjectRequest =
		| { readonly type: "assign"; readonly project_id: string }
		| { readonly type: "browse" }
		| { readonly type: "load" };

	const project_requests = yield* Queue.unbounded<ProjectRequest>();

	/** Sentinel for the item that leads to the folder picker; never a real project id. */
	const BROWSE_VALUE = "artisan:browse-for-folder";

	const RequestProject = (value: string) =>
		Queue.offerUnsafe(
			project_requests,
			value === BROWSE_VALUE ? { type: "browse" } : { project_id: value, type: "assign" },
		);

	const RequestProjectList = (is_open: boolean) => {
		if (is_open) Queue.offerUnsafe(project_requests, { type: "load" });
	};

	const SelectServerProjectDirectory = (directory_id: ProjectDirectoryId) =>
		Effect.gen(function* () {
			yield* Effect.sync(() => {
				assigning = true;
			});
			const selected = yield* client.SelectProjectDirectory({ directory_id });
			yield* AssignProject(selected);
		}).pipe(
			Effect.ensuring(
				Effect.sync(() => {
					assigning = false;
				}),
			),
			Effect.catch((error) =>
				banner.error("Could not select project folder", { description: error.message }),
			),
		);

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
		yield* Effect.sync(() => {
			existing_projects = catalog.projects;
		});

		/**
		 * Repository state is read separately so a slow or missing checkout never
		 * delays the project names themselves.
		 */
		const result = yield* client.GetProjectRepositories().pipe(
			Effect.retry({ schedule: ColdStartRetrySchedule }),
			Effect.catch(() => Effect.succeed({ repositories: [] })),
		);
		yield* Effect.sync(() => {
			repositories = new Map(
				result.repositories.map((entry) => [entry.project_id, entry.repository]),
			);
		});
	}).pipe(
		Effect.catch((error) => banner.error("Could not load projects", { description: error.message })),
	);

	/** Absent `project_id` is a request in its own right: clear, and read nothing. */
	type DiffRequest = { readonly project_id: string | undefined };

	const diff_requests = yield* Queue.unbounded<DiffRequest>();
	const lip_arm_requests = yield* Queue.sliding<void>(1);
	const NextAnimationFrame = Effect.callback<void>((resume) => {
		const frame = requestAnimationFrame(() => resume(Effect.void));
		return Effect.sync(() => cancelAnimationFrame(frame));
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
		client.GetProjectDiffs([requested]).pipe(
			Effect.map(
				(result) => result.diffs.find((entry) => entry.project_id === requested)?.diff,
			),
			Effect.flatMap((snapshot) => ApplyDiff(requested, snapshot)),
			/** A diff is a nicety; failing to read one must not raise a banner over the thread. */
			Effect.catch(() => ApplyDiff(requested, undefined)),
		);

	/**
	 * Clearing runs on the queue's own fiber rather than in the effect that asks
	 * for it, so no renderer state is written during the render pass.
	 */
	const HandleDiffRequest = (request: DiffRequest) =>
		(request.project_id === undefined
			? ApplyDiff(undefined, undefined)
			: ApplyDiff(request.project_id, undefined).pipe(
					Effect.andThen(LoadDiff(request.project_id)),
				)
		).pipe(
			Effect.andThen(
				Effect.sync(() => {
					/** A settle for a project since left must not arm the next one's reveal. */
					if (request.project_id === project?.project_id) diff_settled = true;
				}),
			),
		);

	const HandleProjectRequest = (request: ProjectRequest) => {
		if (request.type === "load") return LoadProjects;
		if (request.type === "browse") return OpenProjectPicker();

		const candidate = existing_projects.find(
			(project_candidate) => project_candidate.project_id === request.project_id,
		);
		return candidate === undefined ? Effect.void : AssignProject(candidate);
	};

	const OpenPanel = Effect.gen(function* () {
		if (close_fiber !== undefined) yield* Fiber.interrupt(close_fiber);
		panel_state = "open";
	});

	const ClosePanel = Effect.gen(function* () {
		panel_state = "closing";
		close_fiber = yield* Effect.forkScoped(
			Effect.sleep("150 millis").pipe(
				Effect.andThen(Effect.sync(() => {
					panel_state = "closed";
				})),
			),
		);
	});

	const TogglePanel = Effect.gen(function* () {
		if (panel_state === "open") yield* ClosePanel;
		else yield* OpenPanel;
	});

	const HandleKeydown = (event: KeyboardEvent) =>
		event.key === "Escape" && panel_state === "open" ? ClosePanel : Effect.void;

	yield* Queue.take(project_requests).pipe(
		Effect.flatMap(HandleProjectRequest),
		Effect.forever,
		Effect.forkScoped,
	);

	yield* Queue.take(diff_requests).pipe(
		Effect.flatMap(HandleDiffRequest),
		Effect.forever,
		Effect.forkScoped,
	);

	/**
	 * Read at mount, not on first open. The header shows the pinned project's
	 * branch and remote, so waiting for the select to be opened left it showing a
	 * bare filesystem path — and fetching on open also reflows the menu under the
	 * pointer as the rows splice in.
	 */
	yield* Effect.forkScoped(LoadProjects);

	/**
	 * Keyed on the identifier, not the project object: the thread's project is a
	 * fresh object on every metadata update, and re-reading on ordinary thread
	 * activity would spend a dozen Git processes to re-derive the same numbers.
	 */
	$effect(() => {
		/** Arriving on a project is a fresh mount for the lip: disarm until its reading paints. */
		diff_settled = false;
		lip_animate = false;
		Queue.offerUnsafe(diff_requests, { project_id: pinned_project_id });
	});

	/**
	 * Two frames, not one: the settled open state and the arming write could
	 * otherwise land in the same style recalc, which would play the reveal the
	 * gate exists to suppress. The first frame paints the state, the second arms.
	 */
	yield* Queue.take(lip_arm_requests).pipe(
		Effect.flatMap(() =>
			NextAnimationFrame.pipe(
				Effect.andThen(NextAnimationFrame),
				Effect.andThen(
					Effect.sync(() => {
						if (diff_settled && !lip_animate) lip_animate = true;
					}),
				),
			),
		),
		Effect.forever,
		Effect.forkScoped,
	);
	$effect(() => {
		if (!diff_settled || lip_animate) return;
		Queue.offerUnsafe(lip_arm_requests, undefined);
	});

	yield* RefreshThreads;

	yield* RunAuthoritativeSubscription(
		client.SubscribeThreadList,
		ApplyThreadListUpdate,
		RefreshThreads,
	).pipe(Effect.forkScoped);
</script>

<svelte:window onkeydown={yield* HandleKeydown(event)} />

<Dialog.Root bind:open={picker_open}>
	<Dialog.Content class="sm:max-w-lg">
		<Dialog.Header>
			<Dialog.Title>Choose a folder</Dialog.Title>
			<Dialog.Description>
				Pick a folder on the machine running Artisan Forge to start a new project.
				Projects you already use are listed in the panel's project menu.
			</Dialog.Description>
		</Dialog.Header>

		<div class="flex min-h-64 flex-col gap-2">
			{#if project_directory_history.length > 0}
				<Button
					variant="ghost"
					class="w-fit"
					disabled={assigning}
					onclick={yield* BrowseParentProjectDirectory()}
				>
					<ChevronLeft data-icon="inline-start" />
					Back
				</Button>
			{/if}

			<ScrollArea.Root class="h-80">
				<div class="flex flex-col gap-1 pr-3">
					{#each project_directories?.directories ?? [] as directory (directory.directory_id)}
						<div class="flex items-center gap-2">
							<Button
								variant="ghost"
								class="min-w-0 grow justify-start"
								disabled={assigning || !directory.has_children}
								onclick={yield* BrowseProjectDirectory(directory.directory_id)}
							>
								<Folder data-icon="inline-start" />
								<span class="truncate">{directory.display_name}</span>
								{#if directory.has_children}
									<ChevronRight class="ml-auto" />
								{/if}
							</Button>
							<Button
								variant="outline"
								disabled={assigning}
								onclick={yield* SelectServerProjectDirectory(directory.directory_id)}
							>
								Select
							</Button>
						</div>
					{:else}
						{#if assigning}
							<p class="text-sm text-muted-foreground">Loading folders...</p>
						{:else}
							<p class="text-sm text-muted-foreground">
								No project folders are available from this backend.
							</p>
						{/if}
					{/each}
				</div>
			</ScrollArea.Root>
		</div>
	</Dialog.Content>
</Dialog.Root>

<!--
	`subject` is a Project and `repository` its ProjectRepository. Both parameters
	are deliberately unannotated: the effect
	runtime's template transform rejects a type annotation on a snippet parameter
	in this component, so adding one fails the build rather than the typecheck.
-->
{#snippet repository_line(subject, repository)}
	{#if repository === undefined || repository.state !== "repository"}
		<!-- Outside version control the path is still the most useful thing to show. -->
		<span class="truncate text-sm text-muted-foreground">
			{ShortProjectPath(subject.root_path, subject.display_name) ?? subject.root_path}
		</span>
	{:else}
		{@const remote = repository.remotes.find((candidate) => candidate.name === repository.default_remote)}
		{@const mark = RepositoryMarkFor(remote?.host)}
		{@const MarkIcon = mark.icon}
		<span class="flex min-w-0 items-center gap-1.5 text-sm text-muted-foreground">
			<MarkIcon class={RepositoryMarkClass(mark, "size-3.5")} />
			{#if remote?.web_url !== undefined}
				<!--
					A repository page is an external destination, so the link opens in the
					browser and is kept out of the select's own click handling.
				-->
				<a
					href={remote.web_url}
					target="_blank"
					rel="noreferrer"
					class="truncate text-(--banner-info) underline-offset-2 transition-colors duration-(--duration-fast) ease-in-out hover:underline motion-reduce:transition-none"
					onclick={(event) => event.stopPropagation()}
				>
					{RepositoryLinkLabel(remote.web_url)}
				</a>
			{:else}
				<span class="truncate">Local</span>
			{/if}
			<span class="shrink-0">on</span>
			<GitBranch class="size-3.5 shrink-0" />
			<span class="truncate">
				{repository.branch.type === "detached" ? "detached HEAD" : repository.branch.name}
			</span>
		</span>
	{/if}
{/snippet}

<div class="relative flex h-full min-h-0 flex-col p-4">
	<header class="mb-2" aria-label="Thread project">
		<Select
			type="single"
			value={project?.project_id ?? ""}
			disabled={assigning || (!is_draft && thread === undefined)}
			onOpenChange={RequestProjectList}
			onValueChange={RequestProject}
		>
			<!--
				The outer surface is only the shader: it lights trigger and lip alike, so the
				lip reads as the same object rather than a card stacked under one. The trigger
				sits directly on that light with no frosted layer of its own — a second
				backdrop-filter over the one already burning underneath only muddies it.
			-->
			<ShaderGlassSurface
				use_material={false}
				class="w-full rounded-2xl bg-(image:--hover-surface-fill) p-2"
			>
				<!--
					The shader surface is the only card: it carries the fill and the rim, so the
					lip card and the trigger stay bare housings inside it. Give either of them a
					face of its own and the trigger reads as a second card nested in the first.
				-->
				<LipCard
					variant="glass"
					open={reportable !== undefined}
					animate={lip_animate}
					class="w-full rounded-2xl"
				>
					<SelectTrigger
						aria-label="Thread project"
						class="w-full items-center gap-2 rounded-2xl border-transparent bg-transparent p-3 text-left data-[size=default]:h-auto dark:bg-transparent dark:hover:bg-transparent"
					>
						<span class="flex min-w-0 flex-col -space-y-1">
							<span class="truncate text-base font-semibold text-foreground">
								{project?.display_name ?? "No project"}
							</span>
							{#if project === undefined}
								<span class="truncate text-sm text-muted-foreground">
									Select a folder to pin this thread
								</span>
							{:else}
								{@render repository_line(project, active_repository)}
							{/if}
						</span>
					</SelectTrigger>

					{#snippet lip()}
						<!--
							The lip shows uncommitted lines only. Everything it cannot fit — the
							staged split, the unmeasured untracked files, and each branch
							baseline — lives behind the marker rather than beside it.
						-->
						{#if reportable !== undefined}
							<div class="flex items-center gap-2 px-4 pb-2.5 pt-2 text-xs tabular-nums">
								<span class="text-(--diff-added)">
									+{DiffCount(reportable.working.lines_added)}
								</span>
								<span class="text-(--diff-removed)">
									−{DiffCount(reportable.working.lines_deleted)}
								</span>
								{#if reportable.truncated}
									<span class="text-muted-foreground">+</span>
								{/if}

								<Popover bind:open={diff_detail_open}>
									<PopoverTrigger
										aria-label="Diff details"
										class="rounded-full text-muted-foreground outline-none transition-colors duration-(--duration-fast) ease-in-out hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 motion-reduce:transition-none"
											onpointerenter={yield* KeepDiffDetailOpen}
											onpointerleave={yield* CloseDiffDetailSoon}
											onfocusin={yield* KeepDiffDetailOpen}
											onfocusout={yield* CloseDiffDetailSoon}
									>
										<HelpCircle class="size-3.5" />
									</PopoverTrigger>
									<PopoverContent
										variant="bare"
										align="start"
										side="bottom"
										sideOffset={8}
										class="w-auto rounded-2xl"
											onpointerenter={yield* KeepDiffDetailOpen}
											onpointerleave={yield* CloseDiffDetailSoon}
										trapFocus={false}
										onOpenAutoFocus={(event) => {
											/**
											 * Hover drives this surface, so it must never move focus.
											 * The defaults focus the content on open and the trigger on
											 * close; with the trigger's focusin/focusout handlers that
											 * feeds back into an open-close flicker loop.
											 */
											event.preventDefault();
										}}
										onCloseAutoFocus={(event) => {
											event.preventDefault();
										}}
									>
										<ShaderGlassSurface strength="strong" class="w-full rounded-2xl">
											<div
												class="grid grid-cols-[auto_auto_auto] items-baseline gap-x-5 gap-y-1.5 p-3 text-xs tabular-nums"
											>
												<DiffCountsRow
													label="Uncommitted"
													counts={reportable.working}
													trailing={DiffFileCount(reportable.working.file_count)}
												/>
												<DiffCountsRow
													label="Staged"
													counts={reportable.staged}
													trailing={DiffFileCount(reportable.staged.file_count)}
												/>
												<DiffCountsRow
													label="Unstaged"
													counts={reportable.unstaged}
													trailing={DiffFileCount(reportable.unstaged.file_count)}
												/>

												{#each reportable.comparisons as comparison (comparison.kind)}
													<DiffCountsRow
														label={`vs ${comparison.ref}`}
														note={ComparisonLabel(comparison.kind)}
														counts={comparison.counts}
														trailing={`${DiffCount(comparison.ahead)} ahead · ${DiffCount(comparison.behind)} behind`}
													/>
												{/each}

												{#if reportable.untracked_file_count > 0}
													<span class="text-muted-foreground">Untracked</span>
													<span class="col-span-2 whitespace-nowrap text-foreground">
														{DiffFileCount(reportable.untracked_file_count)} — lines not counted
													</span>
												{/if}

												{#if reportable.working.binary_file_count > 0}
													<span class="text-muted-foreground">Binary</span>
													<span class="col-span-2 whitespace-nowrap text-foreground">
														{DiffFileCount(reportable.working.binary_file_count)} — lines not counted
													</span>
												{/if}

												{#if reportable.stash_count > 0}
													<span class="text-muted-foreground">Stashed</span>
													<span class="col-span-2 whitespace-nowrap text-foreground">
														{DiffCount(reportable.stash_count)}
														{reportable.stash_count === 1 ? "entry" : "entries"}
													</span>
												{/if}

												{#if reportable.head_committed_at !== undefined}
													<span class="text-muted-foreground">Last commit</span>
													<span class="col-span-2 whitespace-nowrap text-foreground">
														{FormatRecentThreadTime(reportable.head_committed_at, read_at_ms)}
													</span>
												{/if}

												{#if reportable.truncated}
													<span class="text-muted-foreground">Note</span>
													<span class="col-span-2 whitespace-nowrap text-foreground">
														The diff exceeded the read limit; counts are a floor.
													</span>
												{/if}
											</div>
										</ShaderGlassSurface>
									</PopoverContent>
								</Popover>
							</div>
						{/if}
					{/snippet}
				</LipCard>
			</ShaderGlassSurface>

			<SelectContent
				align="start"
				class="rounded-2xl border-transparent bg-transparent p-0 shadow-none"
			>
				<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
					<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
						{#snippet children({ move_hover })}
							{#each existing_projects as candidate (candidate.project_id)}
								<SelectItem
									value={candidate.project_id}
									label={candidate.display_name}
									class="w-full focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
									{@attach FollowHighlight(move_hover)}
								>
									<span class="flex min-w-0 flex-col -space-y-1">
										<span class="truncate text-sm text-foreground">{candidate.display_name}</span>
										{@render repository_line(candidate, repositories.get(candidate.project_id))}
									</span>
								</SelectItem>
							{/each}

							<SelectItem
								value={BROWSE_VALUE}
								label="Choose a folder"
								class="w-full focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
								{@attach FollowHighlight(move_hover)}
							>
								<Folder class="size-4 shrink-0 text-muted-foreground" />
								<span class="text-sm">Choose a folder…</span>
							</SelectItem>
						{/snippet}
					</DropdownHoverSurface>
				</ShaderGlassSurface>
			</SelectContent>
		</Select>
	</header>

	<Button
		variant="outline"
		size="icon-sm"
		class="absolute right-0 bottom-0 z-30 bg-background/80 backdrop-blur-xl"
		onclick={yield* TogglePanel}
		aria-label="Shader settings"
		aria-controls="shader-development-panel"
		aria-expanded={panel_state === "open"}
	>
		<Settings class="size-4 text-muted-foreground" />
	</Button>

	<div
		id="shader-development-panel"
		class:is-open={panel_state === "open"}
		class:is-closing={panel_state === "closing"}
		class="t-dropdown absolute inset-x-2 top-2 bottom-10 z-20 flex min-h-0 flex-col overflow-hidden bg-background/95 p-3 backdrop-blur-xl card"
		data-origin="bottom-right"
		aria-hidden={panel_state === "closed"}
	>
		<ShaderDevPanel />
	</div>
</div>

<style>
	:global(:root) {
		--dropdown-open-dur: 250ms;
		--dropdown-close-dur: 150ms;
		--dropdown-pre-scale: 0.97;
		--dropdown-closing-scale: 0.99;
		--dropdown-ease: cubic-bezier(0.22, 1, 0.36, 1);
	}

	.t-dropdown {
		transform-origin: top left;
		transform: scale(var(--dropdown-pre-scale));
		opacity: 0;
		pointer-events: none;
		transition:
			transform var(--dropdown-open-dur) var(--dropdown-ease),
			opacity var(--dropdown-open-dur) var(--dropdown-ease);
		will-change: transform, opacity;
	}

	.t-dropdown[data-origin="bottom-right"] {
		transform-origin: bottom right;
	}

	.t-dropdown.is-open {
		transform: scale(1);
		opacity: 1;
		pointer-events: auto;
	}

	.t-dropdown.is-closing {
		transform: scale(var(--dropdown-closing-scale));
		opacity: 0;
		pointer-events: none;
		transition:
			transform var(--dropdown-close-dur) var(--dropdown-ease),
			opacity var(--dropdown-close-dur) var(--dropdown-ease);
	}

	@media (prefers-reduced-motion: reduce) {
		.t-dropdown {
			transition: none !important;
		}
	}
</style>

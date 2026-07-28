<script lang="ts" effect>
	import { dev } from "$app/environment";
	import { page } from "$app/state";
	import ChevronLeft from "@tabler/icons-svelte/icons/chevron-left";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import Folder from "@tabler/icons-svelte/icons/folder";
	import Settings from "@tabler/icons-svelte/icons/settings";
	import { Effect, Fiber, Option } from "effect";
	import type {
		Project,
		ProjectDirectoryId,
		ProjectDirectoryList,
		ThreadListItem,
	} from "@artisan/protocol";
	import { ArtisanClient, type ThreadListUpdate } from "@artisan/transport/client";
	import { BannerService } from "$lib/banner/service";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import { Button } from "$lib/components/ui/button";
	import * as Dialog from "$lib/components/ui/dialog";
	import * as ScrollArea from "$lib/components/ui/scroll-area";
	import { draft_thread_project } from "$lib/root/draft-thread";
	import {
		ApplyRootThreadListUpdate,
		ResolveThreadRoute,
	} from "$lib/root/thread-navigation";
	import ShaderDevPanel from "./shader-dev-panel.sv";

	type PanelState = "closed" | "open" | "closing";

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	let threads = $state.raw<ReadonlyArray<ThreadListItem>>([]);
	let existing_projects = $state.raw<ReadonlyArray<Project>>([]);
	let picker_open = $state(false);
	let assigning = $state(false);
	let project_directories = $state.raw<ProjectDirectoryList | undefined>();
	let project_directory_history = $state.raw<ReadonlyArray<ProjectDirectoryId>>([]);
	let panel_state: PanelState = $state("closed");
	let close_fiber: Fiber.Fiber<void> | undefined;

	const route_id = $derived(page.params.id);
	/** The draft route has no durable thread yet; its project lives in the draft store. */
	const is_draft = $derived(page.url.pathname === "/threads/new");
	/** The panel mounts only on thread routes, so the route id names the thread. */
	const thread = $derived(
		is_draft || route_id === undefined
			? undefined
			: Option.getOrUndefined(ResolveThreadRoute(threads, route_id)),
	);
	const project = $derived(is_draft ? $draft_thread_project : thread?.primary_project);

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
			<Dialog.Title>Select a project</Dialog.Title>
			<Dialog.Description>
				Pin this thread to an existing project or choose a folder on the machine
				running Artisan Forge.
			</Dialog.Description>
		</Dialog.Header>

		<div class="flex min-h-64 flex-col gap-2">
			{#if existing_projects.length > 0}
				<div class="flex flex-col gap-1">
					{#each existing_projects as candidate (candidate.project_id)}
						<Button
							variant="ghost"
							class="min-w-0 justify-start"
							disabled={assigning}
							title={candidate.root_path}
							onclick={yield* AssignProject(candidate)}
						>
							<Folder data-icon="inline-start" />
							<span class="truncate">{candidate.display_name}</span>
						</Button>
					{/each}
				</div>
				<p class="px-2 pt-2 text-xs text-muted-foreground">Or select a folder</p>
			{/if}

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

<div class="relative flex h-full min-h-0 flex-col p-4">
	<header class="mb-2" aria-label="Thread project">
		<Button
			variant="ghost"
			class="w-full min-w-0 justify-start"
			disabled={assigning || (!is_draft && thread === undefined)}
			title={project?.root_path}
			onclick={yield* OpenProjectPicker()}
		>
			<Folder data-icon="inline-start" class="text-muted-foreground" />
			<span class="truncate">{project?.display_name ?? "No project"}</span>
		</Button>
	</header>

	{#if dev}
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
	{/if}
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

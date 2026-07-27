<script lang="ts" effect>
	import { goto } from "$app/navigation";
	import { page } from "$app/state";
	import Folder from "@tabler/icons-svelte/icons/folder";
	import ChevronLeft from "@tabler/icons-svelte/icons/chevron-left";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import MessageCircle from "@tabler/icons-svelte/icons/message-circle";
	import Plus from "@tabler/icons-svelte/icons/plus";
	import ShoppingBag from "@tabler/icons-svelte/icons/shopping-bag";
	import { Effect } from "effect";
	import type {
		Project,
		ProjectDirectoryId,
		ProjectDirectoryList,
		ThreadListItem,
	} from "@artisan/protocol";
	import {
		ArtisanClient,
		type ProjectCatalogUpdate,
		type ThreadListUpdate,
	} from "@artisan/transport/client";
	import { BannerService } from "$lib/banner/service";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import barekey_logo from "$lib/assets/barekey/logo-40.png";
	import { Button } from "$lib/components/ui/button";
	import * as Dialog from "$lib/components/ui/dialog";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
	import * as ScrollArea from "$lib/components/ui/scroll-area";
	import {
		ApplyRootThreadListUpdate,
		ProjectScopedThreadGroups,
		ThreadRoutePath,
	} from "$lib/root/thread-navigation";
	import * as Sidebar from "$lib/components/ui/sidebar";

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	let threads = $state.raw<ReadonlyArray<ThreadListItem>>([]);
	let projects = $state.raw<ReadonlyArray<Project>>([]);
	let creating = $state(false);
	let project_picker_open = $state(false);
	let project_directories = $state.raw<ProjectDirectoryList | undefined>();
	let project_directory_history = $state.raw<ReadonlyArray<ProjectDirectoryId>>([]);
	const project_thread_groups = $derived(ProjectScopedThreadGroups(threads));

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

	const ApplyProjectCatalogUpdate = (update: ProjectCatalogUpdate) =>
		Effect.sync(() => {
			projects = update.snapshot.projects;
		});

	const RefreshProjects = client.ListProjects.pipe(
		Effect.map((snapshot) => ({ snapshot, type: "snapshot" as const })),
		Effect.flatMap(ApplyProjectCatalogUpdate),
	);

	const CreateThreadInProject = (project: Project) =>
		Effect.gen(function* () {
			yield* Effect.sync(() => {
				creating = true;
			});
			const thread = yield* client.CreateThread({
				project_id: project.project_id,
				title: "New thread",
			});
			yield* Effect.promise(() => goto(ThreadRoutePath(thread.thread_id)));
		}).pipe(
			Effect.ensuring(
				Effect.sync(() => {
					creating = false;
				}),
			),
			Effect.catch((error) =>
				banner.error("Could not create thread", { description: error.message }),
			),
		);

	const CreateThreadInMostRecentProject = () => {
		const project = projects[0];
		return project === undefined
			? SelectProjectAndCreateThread()
			: CreateThreadInProject(project);
	};

	const LoadProjectDirectories = (
		parent_directory_id?: ProjectDirectoryId,
		history: ReadonlyArray<ProjectDirectoryId> = [],
	) =>
		Effect.gen(function* () {
			yield* Effect.sync(() => {
				creating = true;
			});
			const listing = yield* client.ListProjectDirectories(
				parent_directory_id === undefined ? undefined : { parent_directory_id },
			);
			yield* Effect.sync(() => {
				project_directories = listing;
				project_directory_history = history;
				project_picker_open = true;
			});
		}).pipe(
			Effect.ensuring(
				Effect.sync(() => {
					creating = false;
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

	const SelectServerProjectDirectory = (directory_id: ProjectDirectoryId) =>
		Effect.gen(function* () {
			yield* Effect.sync(() => {
				creating = true;
			});
			const project = yield* client.SelectProjectDirectory({ directory_id });
			yield* Effect.sync(() => {
				project_picker_open = false;
			});
			yield* CreateThreadInProject(project);
		}).pipe(
			Effect.ensuring(
				Effect.sync(() => {
					creating = false;
				}),
			),
			Effect.catch((error) =>
				banner.error("Could not select project folder", { description: error.message }),
			),
		);

	const SelectProjectAndCreateThread = () =>
		LoadProjectDirectories();

	yield* RefreshThreads;
	yield* RefreshProjects;

	yield* RunAuthoritativeSubscription(
		client.SubscribeThreadList,
		ApplyThreadListUpdate,
		RefreshThreads,
	).pipe(
		Effect.forkScoped,
	);
	yield* RunAuthoritativeSubscription(
		client.SubscribeProjects,
		ApplyProjectCatalogUpdate,
		RefreshProjects,
	).pipe(Effect.forkScoped);
</script>

<Dialog.Root bind:open={project_picker_open}>
	<Dialog.Content class="sm:max-w-lg">
		<Dialog.Header>
			<Dialog.Title>Select a project folder</Dialog.Title>
			<Dialog.Description>
				Choose a folder on the machine running Artisan Forge.
			</Dialog.Description>
		</Dialog.Header>

		<div class="flex min-h-64 flex-col gap-2">
			{#if project_directory_history.length > 0}
				<Button
					variant="ghost"
					class="w-fit"
					disabled={creating}
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
								disabled={creating || !directory.has_children}
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
								disabled={creating}
								onclick={yield* SelectServerProjectDirectory(directory.directory_id)}
							>
								Select
							</Button>
						</div>
					{:else}
						{#if creating}
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

<div class="t-sidebar-flyout flex min-w-0 flex-1 flex-col">
	<Sidebar.Header class="h-14 justify-center pl-6 pr-14 lg:pl-2">
		<div
			class="t-sidebar-flyout-inline inline-block min-w-0 max-w-[var(--sidebar-flyout-inline-width,none)] overflow-visible whitespace-nowrap"
		>
			<a href="/" class="t-sidebar-child -ml-1 flex flex-row items-center gap-2 lg:ml-0">
				<img src={barekey_logo} alt="" class="size-5 shrink-0 invert dark:invert-0" />
				<span class="font-logo">Artisan Editor</span>
			</a>
		</div>
	</Sidebar.Header>

	<Sidebar.Content
		class="docs-sidebar-nav-surface docs-scroll-fade relative overflow-x-hidden px-2 pb-3"
	>
		<Sidebar.Group class="px-0 py-2">
			<Sidebar.GroupContent>
				<div class="flex flex-col gap-2">
					<DropdownMenu.Root>
						<DropdownMenu.Trigger>
							{#snippet child({ props })}
								<Button
									{...props}
									class="w-full justify-start group-data-[collapsible=icon]:size-9 group-data-[collapsible=icon]:px-0"
								>
									<Plus data-icon="inline-start" />
									<span class="group-data-[collapsible=icon]:hidden">New…</span>
								</Button>
							{/snippet}
						</DropdownMenu.Trigger>

						<DropdownMenu.Content side="bottom" align="start">
							<DropdownMenu.Item
								onclick={yield* SelectProjectAndCreateThread()}
							>
								<Folder />
								<span>Project</span>
							</DropdownMenu.Item>
							<DropdownMenu.Item
								onclick={yield* CreateThreadInMostRecentProject()}
							>
								<MessageCircle />
								<span>Thread</span>
							</DropdownMenu.Item>
						</DropdownMenu.Content>
					</DropdownMenu.Root>

					<Button
						variant="ghost"
						class="text-muted-foreground w-full justify-start group-data-[collapsible=icon]:size-9 group-data-[collapsible=icon]:px-0"
					>
						<ShoppingBag data-icon="inline-start" />
						<span class="group-data-[collapsible=icon]:hidden">Marketplace</span>
					</Button>
				</div>
			</Sidebar.GroupContent>
		</Sidebar.Group>

		<Sidebar.Group class="px-0 py-2 group-data-[collapsible=icon]:hidden">
			<Sidebar.GroupContent>
				<Sidebar.Menu class="gap-2">
					{#each project_thread_groups as group (group.type === "project" ? group.project.project_id : "unassigned")}
						{@const project = group.type === "project" ? group.project : undefined}
						<Sidebar.MenuItem>
							<div
								class="flex h-8 min-w-0 items-center gap-2 px-2 text-sm text-foreground"
								title={project?.root_path}
							>
								<Folder class="size-4 shrink-0 text-muted-foreground" />
								<span class="truncate">{project?.display_name ?? "Unassigned"}</span>
							</div>
							<Sidebar.MenuSub
								class="mx-0 translate-x-0 gap-0.5 border-l-0 px-0 py-0"
							>
								{#each group.threads as thread (thread.thread_id)}
									<Sidebar.MenuSubItem>
										<Sidebar.MenuSubButton
											href={ThreadRoutePath(thread.thread_id)}
											isActive={page.url.pathname === ThreadRoutePath(thread.thread_id)}
											class="h-8 translate-x-0 rounded-lg pl-9 pr-2 text-sm"
											title={thread.title}
										>
											<span>{thread.title}</span>
										</Sidebar.MenuSubButton>
									</Sidebar.MenuSubItem>
								{/each}
							</Sidebar.MenuSub>
						</Sidebar.MenuItem>
					{/each}
				</Sidebar.Menu>
			</Sidebar.GroupContent>
		</Sidebar.Group>
	</Sidebar.Content>
</div>

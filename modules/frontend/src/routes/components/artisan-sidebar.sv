<script lang="ts" effect>
	import { page } from "$app/state";
	import Edit from "@tabler/icons-svelte/icons/edit";
	import Folder from "@tabler/icons-svelte/icons/folder";
	import ShoppingBag from "@tabler/icons-svelte/icons/shopping-bag";
	import { Effect } from "effect";
	import type { ThreadListItem } from "@artisan/protocol";
	import { ArtisanClient, type ThreadListUpdate } from "@artisan/transport/client";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import barekey_logo from "$lib/assets/barekey/logo-40.png";
	import { Button } from "$lib/components/ui/button";
	import {
		ApplyRootThreadListUpdate,
		ProjectScopedThreadGroups,
		ThreadRoutePath,
	} from "$lib/root/thread-navigation";
	import * as Sidebar from "$lib/components/ui/sidebar";
	import SidebarIdentity from "./sidebar-identity.sv";

	const client = yield* ArtisanClient;
	let threads = $state.raw<ReadonlyArray<ThreadListItem>>([]);
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

	yield* RefreshThreads;

	yield* RunAuthoritativeSubscription(
		client.SubscribeThreadList,
		ApplyThreadListUpdate,
		RefreshThreads,
	).pipe(
		Effect.forkScoped,
	);
</script>

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
					<Button
						href="/threads/new"
						variant="ghost"
						class="w-full justify-start group-data-[collapsible=icon]:size-9 group-data-[collapsible=icon]:px-0"
					>
						<Edit data-icon="inline-start" />
						<span class="group-data-[collapsible=icon]:hidden">New thread</span>
					</Button>

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

	<Sidebar.Footer class="group-data-[collapsible=icon]:hidden">
		<SidebarIdentity />
	</Sidebar.Footer>
</div>

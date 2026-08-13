<script lang="ts" effect>
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import DeviceDesktop from "@tabler/icons-svelte/icons/device-desktop";
	import Download from "@tabler/icons-svelte/icons/download";
	import File from "@tabler/icons-svelte/icons/file";
	import FileText from "@tabler/icons-svelte/icons/file-text";
	import Folder from "@tabler/icons-svelte/icons/folder";
	import FolderPlus from "@tabler/icons-svelte/icons/folder-plus";
	import Home from "@tabler/icons-svelte/icons/home";
	import Music from "@tabler/icons-svelte/icons/music";
	import Photo from "@tabler/icons-svelte/icons/photo";
	import Search from "@tabler/icons-svelte/icons/search";
	import Server from "@tabler/icons-svelte/icons/server";
	import Video from "@tabler/icons-svelte/icons/video";
	import { Effect } from "effect";
	import type {
		Project,
		ProjectDirectoryEntry,
		ProjectDirectoryId,
		ProjectDirectoryList,
		ProjectDirectoryPlace,
		ProjectDirectoryPlaceKind,
	} from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import { BannerService } from "$lib/banner/service";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { Button } from "$lib/components/ui/button";
	import { MakeLatestRequestGate } from "$lib/lifecycle/latest-request-gate";
	import * as ContextMenu from "$lib/components/ui/context-menu";
	import * as Dialog from "$lib/components/ui/dialog";
	import * as InputGroup from "$lib/components/ui/input-group";
	import DropdownHoverSurface from "./dropdown-hover-surface.svelte";

	let {
		open = $bindable(false),
		onselect,
	}: {
		open?: boolean;
		/** Receives the resolved project once a folder has been chosen. */
		onselect: (project: Project) => Effect.Effect<unknown>;
	} = $props();

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	const browse_requests = yield* MakeLatestRequestGate;

	/** One breadcrumb segment: the id to return to and the name it showed as. */
	type Crumb = {
		readonly directory_id: ProjectDirectoryId;
		readonly display_name: string;
	};

	let listing = $state.raw<ProjectDirectoryList | undefined>();
	let places = $state.raw<ReadonlyArray<ProjectDirectoryPlace>>([]);
	let trail = $state.raw<ReadonlyArray<Crumb>>([]);
	let loading = $state(false);
	let selecting = $state(false);
	let search = $state("");
	let highlighted_id = $state<ProjectDirectoryId | undefined>();
	/** The folder row under the pointer when the context menu opened, if any. */
	let context_entry = $state.raw<ProjectDirectoryEntry | undefined>();
	let naming_folder = $state(false);
	let new_folder_name = $state("");

	const current_directory_id = $derived(trail.at(-1)?.directory_id);
	const query = $derived(search.trim().toLowerCase());
	const visible_directories = $derived(
		(listing?.directories ?? []).filter(
			(entry) => query === "" || entry.display_name.toLowerCase().includes(query),
		),
	);
	const visible_files = $derived(
		(listing?.files ?? []).filter((name) => query === "" || name.toLowerCase().includes(query)),
	);
	/** The footer selects the highlighted folder, or the folder being looked at. */
	const selectable_id = $derived(highlighted_id ?? current_directory_id);

	const place_icons: Record<ProjectDirectoryPlaceKind, typeof Folder> = {
		desktop: DeviceDesktop,
		documents: FileText,
		downloads: Download,
		home: Home,
		music: Music,
		pictures: Photo,
		videos: Video,
	};

	const Browse = (next_trail: ReadonlyArray<Crumb>) =>
		Effect.gen(function* () {
			const request_epoch = yield* browse_requests.Begin;
			loading = true;
			const parent = next_trail.at(-1)?.directory_id;
			const outcome = yield* Effect.result(
				client.ListProjectDirectories(
					parent === undefined ? undefined : { parent_directory_id: parent },
				),
			);
			if (!(yield* browse_requests.IsCurrent(request_epoch))) return false;
			loading = false;
			if (outcome._tag === "Failure") {
				yield* banner.error("Could not load folders", {
					description: outcome.failure.message,
				});
				return false;
			}
			const result = outcome.success;
			listing = result;
			trail = next_trail;
			if (result.places !== undefined) places = result.places;
			search = "";
			highlighted_id = undefined;
			naming_folder = false;
			return true;
		});

	const EnterDirectory = (entry: ProjectDirectoryEntry) =>
		Effect.gen(function* () {
			yield* Browse([
				...trail,
				{ directory_id: entry.directory_id, display_name: entry.display_name },
			]);
		});

	const JumpToPlace = (place: ProjectDirectoryPlace) =>
		Effect.gen(function* () {
			yield* Browse([{ directory_id: place.directory_id, display_name: place.display_name }]);
		});

	const SelectDirectory = (directory_id: ProjectDirectoryId) =>
		Effect.gen(function* () {
			selecting = true;
			const project = yield* client.SelectProjectDirectory({ directory_id });
			yield* onselect(project);
			open = false;
			return true;
		}).pipe(
			Effect.ensuring(
				Effect.gen(function* () {
					selecting = false;
				}),
			),
			Effect.catch((error) =>
				Effect.gen(function* () {
					yield* banner.error("Could not select folder", { description: error.message });
					return false;
				}),
			),
		);

	/** Selects the highlighted folder, or the open folder when nothing is highlighted. */
	const ConfirmSelection = () =>
		Effect.gen(function* () {
			if (selectable_id !== undefined) yield* SelectDirectory(selectable_id);
		});

	const StartNamingFolder = () =>
		Effect.gen(function* () {
			new_folder_name = "New folder";
			naming_folder = true;
		});

	const SelectFolderName = (input: HTMLInputElement) =>
		Effect.gen(function* () {
			yield* RunBrowserDom(() => input.select());
		});

	const CreateFolder = () =>
		Effect.gen(function* () {
			const parent_directory_id = current_directory_id;
			const name = new_folder_name.trim();
			if (parent_directory_id === undefined || name === "") return;
			loading = true;
			const created = yield* client.CreateProjectDirectory({
				name,
				parent_directory_id,
			});
			const applied = yield* Browse(trail);
			if (applied) highlighted_id = created.directory_id;
		}).pipe(
			Effect.ensuring(
				Effect.gen(function* () {
					loading = false;
				}),
			),
			Effect.catch((error) =>
				Effect.gen(function* () {
					yield* banner.error("Could not create folder", { description: error.message });
				}),
			),
		);

	const HandleNewFolderKey = (event: KeyboardEvent) =>
		Effect.gen(function* () {
			if (event.key === "Escape") {
				yield* RunBrowserDom(() => event.stopPropagation());
				naming_folder = false;
				return;
			}
			if (event.key !== "Enter") return;
			yield* RunBrowserDom(() => event.preventDefault());
			yield* CreateFolder();
		});

	const PickNativeDirectory = Effect.gen(function* () {
		selecting = true;
		const picked = yield* client.PickProjectDirectory.pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					return undefined;
				}),
			),
		);
		selecting = false;
		if (picked === undefined) return false;
		if (picked.status === "cancelled") {
			open = false;
			return true;
		}

		return yield* SelectDirectory(picked.directory.directory_id);
	}).pipe(
		Effect.ensuring(
			Effect.gen(function* () {
				selecting = false;
			}),
		),
	);

	/** Reveal starts at the roots on every open, with stale state cleared first. */
	const InitializeOpenPicker = (is_open: boolean) =>
		Effect.gen(function* () {
			if (!is_open) return;
			listing = undefined;
			trail = [];
			search = "";
			highlighted_id = undefined;
			naming_folder = false;
			const handled = yield* PickNativeDirectory;
			if (!handled && open) {
				yield* Browse([]);
			}
		});
	yield* InitializeOpenPicker(open);

	const ClearContextEntry = () =>
		Effect.gen(function* () {
			context_entry = undefined;
		});

	const OpenContextEntry = (entry: ProjectDirectoryEntry) =>
		Effect.gen(function* () {
			context_entry = entry;
			highlighted_id = entry.directory_id;
		});

	const StopNamingFolder = () =>
		Effect.gen(function* () {
			naming_folder = false;
		});

	const ClosePicker = () =>
		Effect.gen(function* () {
			open = false;
		});
</script>

<Dialog.Root bind:open>
	<Dialog.Content
		class="flex h-[min(34rem,85dvh)] flex-col gap-3 p-3 sm:max-w-2xl"
		showCloseButton={false}
	>
		<!-- Finder-style chrome: the path and search are the header. -->
		<Dialog.Header class="sr-only">
			<Dialog.Title>Choose a folder</Dialog.Title>
			<Dialog.Description>
				Pick a folder on the machine running Artisan Forge to start a new project.
			</Dialog.Description>
		</Dialog.Header>

		<div class="flex items-center gap-3 pt-1 pl-2">
			<nav
				aria-label="Current folder path"
				class="flex h-9 min-w-0 flex-1 items-center gap-1 overflow-x-auto whitespace-nowrap"
			>
				<button
					type="button"
					class="flex shrink-0 items-center gap-1.5 rounded-lg text-sm text-muted-foreground outline-none transition-colors duration-(--duration-fast) ease-in-out hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 disabled:text-foreground motion-reduce:transition-none"
					disabled={loading || selecting || trail.length === 0}
					onclick={yield* Browse([])}
				>
					<Server class="size-3.5 shrink-0" aria-hidden="true" />
					Computer
				</button>
				{#each trail as crumb, index (`${crumb.directory_id}:${index}`)}
					<ChevronRight
						class="size-3.5 shrink-0 text-muted-foreground/50"
						aria-hidden="true"
					/>
					<button
						type="button"
						class={`shrink-0 rounded-lg text-sm outline-none transition-colors duration-(--duration-fast) ease-in-out focus-visible:ring-2 focus-visible:ring-ring/50 motion-reduce:transition-none ${index === trail.length - 1 ? "font-medium text-foreground" : "text-muted-foreground hover:text-foreground"}`}
						aria-current={index === trail.length - 1 ? "location" : undefined}
						disabled={loading || selecting || index === trail.length - 1}
						onclick={yield* Browse(trail.slice(0, index + 1))}
					>
						{crumb.display_name}
					</button>
				{/each}
			</nav>
			<InputGroup.Root class="h-9 w-48 shrink-0 bg-surface-100 dark:bg-surface-900">
				<InputGroup.Input
					bind:value={search}
					placeholder="Search"
					aria-label="Search this folder"
				/>
				<InputGroup.Addon>
					<Search class="size-4 shrink-0 opacity-50" aria-hidden="true" />
				</InputGroup.Addon>
			</InputGroup.Root>
		</div>

		<div class="flex min-h-0 flex-1 gap-2">
			<!-- The usual places, straight from the machine running Forge. -->
			{#if places.length > 0}
				<aside class="w-40 shrink-0 py-1 pr-1" aria-label="Places">
					<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
						{#snippet children({ move_hover })}
							<div class="flex flex-col gap-0.5">
								{#each places as place (place.directory_id)}
									{@const PlaceIcon = place_icons[place.place]}
									{@const is_active = current_directory_id === place.directory_id}
									<button
										type="button"
										class={`flex items-center gap-2 rounded-xl px-2.5 py-1.5 text-left text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:opacity-45 ${is_active ? "font-medium text-foreground bg-(image:--hover-surface-fill)" : "text-muted-foreground"}`}
										aria-current={is_active ? "location" : undefined}
										disabled={loading || selecting}
										onpointerenter={move_hover}
										onpointermove={move_hover}
										onfocusin={move_hover}
										onclick={yield* JumpToPlace(place)}
									>
										<PlaceIcon class="size-4 shrink-0" aria-hidden="true" />
										<span class="truncate">{place.display_name}</span>
									</button>
								{/each}
							</div>
						{/snippet}
					</DropdownHoverSurface>
				</aside>
			{/if}

			<!-- The folder's actual contents; files give context but never select. -->
			<ContextMenu.Root>
				<ContextMenu.Trigger class="min-h-0 min-w-0 flex-1">
					<section
						class="h-full rounded-3xl bg-linear-to-b from-surface-125 to-surface-75 p-1 card dark:from-surface-900 dark:to-surface-925"
						aria-label="Folder contents"
					>
						{#if listing === undefined}
							<p class="flex h-full items-center justify-center text-sm text-muted-foreground">
								Loading folders…
							</p>
						{:else if visible_directories.length === 0 && visible_files.length === 0 && !naming_folder}
							<p class="flex h-full items-center justify-center text-sm text-muted-foreground">
								{query === "" ? "This folder is empty." : "Nothing matches your search."}
							</p>
						{:else}
							<div
						class="picker-scroll docs-scroll-fade h-full overflow-x-hidden overflow-y-auto p-1"
						oncontextmenucapture={yield* ClearContextEntry()}
							>
								<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
									{#snippet children({ move_hover })}
										<div class="flex flex-col gap-0.5">
											{#if naming_folder}
												<div
													class="flex w-full items-center gap-2 rounded-xl bg-(image:--hover-surface-fill) px-2.5 py-1.5 text-sm"
												>
													<FolderPlus
														class="size-4 shrink-0 text-muted-foreground"
														aria-hidden="true"
													/>
													<!-- svelte-ignore a11y_autofocus -->
													<input
														type="text"
														aria-label="New folder name"
														class="min-w-0 flex-1 bg-transparent text-foreground outline-none placeholder:text-muted-foreground"
												bind:value={new_folder_name}
												autofocus
												onfocus={yield* SelectFolderName(event.currentTarget)}
												onkeydown={yield* HandleNewFolderKey(event)}
												onblur={yield* StopNamingFolder()}
													/>
												</div>
											{/if}
											{#each visible_directories as entry (entry.directory_id)}
												<button
													type="button"
													class={`flex w-full items-center gap-2 rounded-xl px-2.5 py-1.5 text-left text-sm text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:opacity-45 ${highlighted_id === entry.directory_id ? "bg-(image:--hover-surface-fill)" : ""}`}
													aria-pressed={highlighted_id === entry.directory_id}
													disabled={loading || selecting}
													onpointerenter={move_hover}
													onpointermove={move_hover}
													onfocusin={move_hover}
											onclick={yield* EnterDirectory(entry)}
											oncontextmenu={yield* OpenContextEntry(entry)}
												>
													<Folder
														class="size-4 shrink-0 text-muted-foreground"
														aria-hidden="true"
													/>
													<span class="min-w-0 flex-1 truncate">{entry.display_name}</span>
													{#if entry.has_children}
														<ChevronRight
															class="size-3.5 shrink-0 text-muted-foreground/40"
															aria-hidden="true"
														/>
													{/if}
												</button>
											{/each}
											{#each visible_files as name (name)}
												<div
													class="flex w-full items-center gap-2 rounded-xl px-2.5 py-1.5 text-sm text-muted-foreground/70"
												>
													<File
														class="size-4 shrink-0 text-muted-foreground/50"
														aria-hidden="true"
													/>
													<span class="min-w-0 flex-1 truncate">{name}</span>
												</div>
											{/each}
										</div>
									{/snippet}
								</DropdownHoverSurface>
							</div>
						{/if}
					</section>
				</ContextMenu.Trigger>
				<ContextMenu.Content class="w-52">
					{#if context_entry !== undefined}
						{@const entry = context_entry}
						<ContextMenu.Item onSelect={yield* EnterDirectory(entry)}>
							<Folder class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
							Open
						</ContextMenu.Item>
						<ContextMenu.Item onSelect={yield* SelectDirectory(entry.directory_id)}>
							<ChevronRight
								class="size-4 shrink-0 text-muted-foreground"
								aria-hidden="true"
							/>
							Select this folder
						</ContextMenu.Item>
						<ContextMenu.Separator />
					{/if}
					<ContextMenu.Item
						disabled={current_directory_id === undefined || loading || selecting}
						onSelect={yield* StartNamingFolder()}
					>
						<FolderPlus class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
						New folder
					</ContextMenu.Item>
				</ContextMenu.Content>
			</ContextMenu.Root>
		</div>

		<div class="flex items-center justify-end gap-2">
			<Button variant="ghost" disabled={selecting} onclick={yield* ClosePicker()}>
				Cancel
			</Button>
			<Button
				disabled={selecting || loading || selectable_id === undefined}
				onclick={yield* ConfirmSelection()}
			>
				Select folder
			</Button>
		</div>
	</Dialog.Content>
</Dialog.Root>

<style>
	/** The model picker's scrollbar: thin, muted, and holding its own gutter. */
	.picker-scroll {
		scrollbar-width: thin;
		scrollbar-color: var(--surface-500) transparent;
	}
</style>

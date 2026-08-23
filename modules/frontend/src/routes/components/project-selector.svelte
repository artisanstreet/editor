<script lang="ts" effect>
	/**
	 * The project the surface is pointed at, and the switch between them.
	 *
	 * It reads as a row rather than a control: the project's identity mark,
	 * the name, and the chevron that says there are others. Pressing it does not
	 * take you anywhere — it repoints the surface you are already on — so it
	 * stays a quiet foot to its pane instead of a primary button. Everything
	 * below the trigger wears the effort selector's dropdown exactly, so a menu
	 * opened here and one opened from the composer's control row are visibly the
	 * same object.
	 */
	import Check from "@tabler/icons-svelte/icons/check";
	import FolderPlus from "@tabler/icons-svelte/icons/folder-plus";
	import Selector from "@tabler/icons-svelte/icons/selector";
	import { Effect, Stream } from "effect";
	import type { Snippet } from "svelte";
	import {
		ProjectIdentityMaximumProjects,
		type Project,
		type ProjectIdentitySource,
	} from "@artisan/protocol";
	import {
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuItem,
		DropdownMenuTrigger,
	} from "$lib/components/ui/dropdown-menu";
	import { MakeFollowHighlight } from "$lib/components/dropdown-highlight";
	import { ProjectIdentityController } from "$lib/root/project-identity-controller";
	import { type RecentProject } from "$lib/root/project-catalog";
	import { ShortProjectPath } from "$lib/root/project-path";
	import { path_separator } from "$lib/appearance-config";
	import { FormatPathSeparators } from "$lib/appearance/display-format";
	import DropdownHoverSurface from "./dropdown-hover-surface.svelte";
	import type { PillHover } from "./hover-pill.svelte";
	import ProjectIdentityMark from "./project-identity-mark.svelte";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";

	const FollowHighlight = yield* MakeFollowHighlight;

	let {
		disabled = false,
		hover,
		onnewproject,
		onselect,
		project,
		projects,
		trigger,
		trigger_label,
	}: {
		disabled?: boolean;
		hover: PillHover;
		/** Attaching a folder, which the surface owns because it owns the dialog. */
		onnewproject: Effect.Effect<void>;
		onselect: (project: Project) => Effect.Effect<void>;
		/** The project the surface currently points at, if the catalog has one. */
		project?: Project;
		/** The attached catalog, freshest first. */
		projects: ReadonlyArray<RecentProject>;
		/**
		 * A caller-shaped face for the same menu. The menu is one object wherever
		 * it opens; only what you press to open it belongs to the surface it sits
		 * on, so a caller that is not a quiet row hands its own in.
		 */
		trigger?: Snippet;
		/** The accessible name a custom trigger answers to. */
		trigger_label?: string;
	} = $props();

	let open = $state(false);
	let selected_project_item: HTMLElement | undefined;
	const identity_controller = yield* ProjectIdentityController;
	let identities = $state.raw<ReadonlyMap<string, ProjectIdentitySource>>(
		yield* identity_controller.Current,
	);
	const ApplyIdentities = (next: ReadonlyMap<string, ProjectIdentitySource>) =>
		Effect.sync(() => {
			identities = next;
		});
	yield* identity_controller.Changes.pipe(
		Stream.runForEach(ApplyIdentities),
		Effect.forkScoped,
	);
	yield* identity_controller
		.Refresh(
			projects
				.slice(0, ProjectIdentityMaximumProjects)
				.map(({ project: recent }) => recent.project_id),
		)
		.pipe(Effect.ignore, Effect.forkScoped);

	/**
	 * With nothing attached this stays an invitation rather than a button
	 * labelled "None": the row still reads, and pressing it offers the only
	 * thing that can be done about it.
	 */
	const label = $derived(project?.display_name ?? "Choose a project");
	const identity = $derived(
		project === undefined ? undefined : identities.get(project.project_id),
	);

	const Choose = (next: Project) =>
		Effect.gen(function* () {
			open = false;
			if (next.project_id === project?.project_id) return;
			yield* onselect(next);
		});

	const NewProject = Effect.gen(function* () {
		open = false;
		yield* onnewproject;
	});

	/**
	 * Dropdown menus otherwise focus their first item when they open. Capture the
	 * row representing the current project so Bits' initial highlighted item and
	 * the shared hover pill both begin on the actual selection instead.
	 */
	const CaptureSelectedProject = (selected: boolean) => (node: HTMLElement) => {
		if (!selected) return;
		selected_project_item = node;
		return () => {
			if (selected_project_item === node) selected_project_item = undefined;
		};
	};

	const FocusSelectedProject = (event: Event) => {
		if (selected_project_item === undefined) return;
		event.preventDefault();
		selected_project_item.focus({ preventScroll: true });
	};
</script>

<DropdownMenu bind:open>
	<DropdownMenuTrigger
		{disabled}
		aria-label={trigger === undefined ? `Project: ${label}` : (trigger_label ?? label)}
		class={trigger === undefined
			? "relative flex w-full min-w-0 items-center gap-2.5 rounded-lg px-2 py-2 text-left text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:pointer-events-none"
			: "w-full rounded-lg outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:pointer-events-none"}
		onpointerenter={hover.move}
		onpointermove={hover.move}
		onfocusin={hover.move}
	>
		{#if trigger !== undefined}
			{@render trigger()}
		{:else}
			<ProjectIdentityMark {identity} />
			<span class="min-w-0 flex-1 truncate text-foreground">{label}</span>
			<Selector class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
		{/if}
	</DropdownMenuTrigger>

	<!--
		Opens upward off its leading edge: the trigger is the foot of its pane, so
		a menu dropped below it would have nowhere to go, and the names differ in
		length, which would step a centred card sideways on every switch.
	-->
	<DropdownMenuContent
		align="start"
		side="top"
		sideOffset={10}
		onOpenAutoFocus={FocusSelectedProject}
		class="t-dropdown w-[min(20rem,calc(100vw-2rem))] rounded-2xl bg-transparent! p-0! shadow-none! ring-0! animate-none!"
	>
		<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
			<!--
				Keep the last row's geometry while focus crosses the separator. Without
				this, Bits briefly returns focus to the menu between the final project
				and New project, clearing the pill so the next row fades in instead of
				sliding from the row it left. Closing the dropdown unmounts the surface.
			-->
			<DropdownHoverSurface hold class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
				{#snippet children({ move_hover })}
					<div class="flex min-w-0 flex-col">
						{#each projects as recent (recent.project.project_id)}
							{@const chosen = recent.project.project_id === project?.project_id}
							{@const recent_identity = identities.get(recent.project.project_id)}
							{@const compact_path = ShortProjectPath(
								recent.project.root_path,
								recent.project.display_name,
								$path_separator,
							)}
							<DropdownMenuItem
								class="relative w-full rounded-xl px-2 py-1.5 focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
								onSelect={yield* Choose(recent.project)}
								onpointerenter={move_hover}
								onpointermove={move_hover}
								onfocusin={move_hover}
								{@attach FollowHighlight(move_hover)}
								{@attach CaptureSelectedProject(chosen)}
							>
								<ProjectIdentityMark identity={recent_identity} />
								<span class="flex min-w-0 flex-1 flex-col">
									<span class="min-w-0 truncate text-foreground">
										{recent.project.display_name}
									</span>
									{#if compact_path !== undefined}
										<span
											class="min-w-0 truncate font-mono text-[0.6875rem] text-muted-foreground"
											title={FormatPathSeparators(
												recent.project.root_path,
												$path_separator,
											)}
										>
											{compact_path}
										</span>
									{/if}
								</span>
								{#if chosen}
									<Check class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
								{/if}
							</DropdownMenuItem>
						{/each}

						<!--
							The rule is what makes the last row a different kind of thing:
							everything above it picks something that already exists, and this
							one goes and makes one. Same hairline the effort selector's own
							groups are parted by.
						-->
						<span class="pointer-events-none -mx-1 my-1 h-px bg-border/50" aria-hidden="true"
						></span>

						<DropdownMenuItem
							class="relative w-full rounded-xl px-2 py-1.5 focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
							onSelect={yield* NewProject}
							onpointerenter={move_hover}
							onpointermove={move_hover}
							onfocusin={move_hover}
							{@attach FollowHighlight(move_hover)}
						>
							<span
								aria-hidden="true"
								class="grid size-6 shrink-0 place-items-center rounded-sm text-muted-foreground"
							>
								<FolderPlus class="size-4" />
							</span>
							<span class="min-w-0 flex-1 truncate text-foreground">New project</span>
						</DropdownMenuItem>
					</div>
				{/snippet}
			</DropdownHoverSurface>
		</ShaderGlassSurface>
	</DropdownMenuContent>
</DropdownMenu>

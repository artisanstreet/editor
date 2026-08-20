<script lang="ts" effect>
	/**
	 * The project, as a word in a sentence.
	 *
	 * It is underlined the way a form field is underlined rather than the way a
	 * link is — dotted, and in the sentence's own colour — because pressing it
	 * does not take you anywhere: it changes the word. Everything below the
	 * trigger wears the effort selector's dropdown exactly, so a menu opened
	 * from prose and a menu opened from the composer's control row are visibly
	 * the same object.
	 */
	import Check from "@tabler/icons-svelte/icons/check";
	import FolderPlus from "@tabler/icons-svelte/icons/folder-plus";
	import { Effect } from "effect";
	import type { Project } from "@artisan/protocol";
	import { Popover, PopoverContent, PopoverTrigger } from "$lib/components/ui/popover";
	import { GradientAvatarColorFor } from "$lib/identity/gradient-avatar";
	import { ProjectMonogram, type RecentProject } from "$lib/root/project-catalog";
	import DropdownHoverSurface from "./dropdown-hover-surface.svelte";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";

	let {
		disabled = false,
		onnewproject,
		onselect,
		project,
		projects,
	}: {
		disabled?: boolean;
		/** Attaching a folder, which the surface owns because it owns the dialog. */
		onnewproject: Effect.Effect<void>;
		onselect: (project: Project) => Effect.Effect<void>;
		/** The project the sentence currently names, if the catalog has one to name. */
		project?: Project;
		/** The attached catalog, freshest first. */
		projects: ReadonlyArray<RecentProject>;
	} = $props();

	let open = $state(false);

	/**
	 * The word itself. With nothing attached it stays a noun rather than becoming
	 * a button labelled "None": the sentence still reads, and pressing it offers
	 * the only thing that can be done about it.
	 */
	const label = $derived(project?.display_name ?? "a project");

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
</script>

<Popover bind:open>
	<PopoverTrigger
		{disabled}
		aria-label={`Project: ${label}`}
		class="rounded-sm text-foreground-extra underline decoration-muted-foreground decoration-dotted decoration-1 underline-offset-[0.28em] outline-none transition-colors duration-(--duration-fast) ease-in-out hover:decoration-foreground-extra focus-visible:ring-2 focus-visible:ring-ring/50 disabled:pointer-events-none motion-reduce:transition-none data-[state=open]:decoration-foreground-extra"
	>
		{label}
	</PopoverTrigger>

	<!--
		Anchored to the word's leading edge: the names differ in length, so a
		centred card would step sideways every time the sentence changed.
	-->
	<PopoverContent
		variant="bare"
		align="start"
		side="bottom"
		sideOffset={10}
		class="t-dropdown w-[min(20rem,calc(100vw-2rem))] rounded-2xl animate-none!"
	>
		<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
			<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
				{#snippet children({ move_hover })}
					<div class="flex min-w-0 flex-col">
						{#each projects as recent (recent.project.project_id)}
							{@const tone = GradientAvatarColorFor(recent.project.project_id)}
							{@const chosen = recent.project.project_id === project?.project_id}
							<button
								type="button"
								class="relative flex min-w-0 items-center gap-2.5 rounded-xl px-2 py-1.5 text-left text-sm outline-none"
								onpointerenter={move_hover}
								onpointermove={move_hover}
								onfocusin={move_hover}
								onclick={yield* Choose(recent.project)}
							>
								<!-- The same square the workspace wears, at the size a row can hold. -->
								<span
									aria-hidden="true"
									class="grid size-6 shrink-0 place-items-center rounded-lg text-[0.625rem] font-medium tracking-wide text-foreground-extra"
									style:background={`linear-gradient(160deg, oklch(from ${tone.from} l c h / 24%), oklch(from ${tone.to} l c h / 7%)), var(--surface-875)`}
									style:box-shadow={`inset 0 0 0 0.5px oklch(from ${tone.to} l c h / 24%)`}
								>
									{ProjectMonogram(recent.project.display_name)}
								</span>
								<span class="flex min-w-0 flex-1 flex-col">
									<span class="min-w-0 truncate text-foreground">
										{recent.project.display_name}
									</span>
									<span class="min-w-0 truncate font-mono text-[0.6875rem] text-surface-600">
										{recent.project.root_path}
									</span>
								</span>
								{#if chosen}
									<Check class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
								{/if}
							</button>
						{/each}

						<!--
							The rule is what makes the last row a different kind of thing:
							everything above it picks something that already exists, and this
							one goes and makes one. Same hairline the effort selector's own
							groups are parted by.
						-->
						{#if projects.length > 0}
							<span class="pointer-events-none -mx-1 my-1 h-px bg-border/50" aria-hidden="true"
							></span>
						{/if}

						<button
							type="button"
							class="relative flex min-w-0 items-center gap-2.5 rounded-xl px-2 py-1.5 text-left text-sm outline-none"
							onpointerenter={move_hover}
							onpointermove={move_hover}
							onfocusin={move_hover}
							onclick={yield* NewProject}
						>
							<span
								aria-hidden="true"
								class="grid size-6 shrink-0 place-items-center rounded-lg text-muted-foreground"
							>
								<FolderPlus class="size-4" />
							</span>
							<span class="min-w-0 flex-1 truncate text-foreground">New project</span>
						</button>
					</div>
				{/snippet}
			</DropdownHoverSurface>
		</ShaderGlassSurface>
	</PopoverContent>
</Popover>

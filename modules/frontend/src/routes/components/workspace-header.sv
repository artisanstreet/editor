<script lang="ts" effect>
	import Folder from "@tabler/icons-svelte/icons/folder";
	import GitBranch from "@tabler/icons-svelte/icons/git-branch";
	import { Effect, Schedule } from "effect";
	import type { Project, ProjectRepository } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import { RepositoryQualifiedLabel } from "$lib/vcs/labels";
	import { RepositoryMarkClass, RepositoryMarkFor } from "$lib/vcs/presentation";

	let {
		project,
	}: {
		/** The workspace the open route is inside; the header renders nothing without one. */
		project: Project | undefined;
	} = $props();

	const client = yield* ArtisanClient;

	/**
	 * The header mounts with the shell, before the transport is necessarily
	 * ready, and a swallowed cold-start failure would leave it naming folders
	 * instead of branches for the whole session — the same trap the thread
	 * panel documents. Retry through the cold start, then give up quietly: a
	 * header is not worth a banner.
	 */
	let repositories = $state.raw<ReadonlyMap<string, ProjectRepository>>(new Map());
	const ColdStartRetrySchedule = Schedule.exponential("100 millis").pipe(
		Schedule.upTo({ duration: "5 seconds" }),
	);
	const LoadRepositories = Effect.gen(function* () {
		const result = yield* client
			.GetProjectRepositories()
			.pipe(Effect.retry({ schedule: ColdStartRetrySchedule }));
		repositories = new Map(
			result.repositories.map((entry) => [entry.project_id, entry.repository]),
		);
	}).pipe(Effect.ignore);
	yield* LoadRepositories.pipe(Effect.forkScoped);

	const repository = $derived(
		project === undefined ? undefined : repositories.get(project.project_id),
	);
</script>

{#if project !== undefined}
	{#if repository !== undefined && repository.state === "repository"}
		{@const remote = repository.remotes.find(
			(candidate) => candidate.name === repository.default_remote,
		)}
		<span class="flex min-w-0 items-center gap-1.5 text-sm text-muted-foreground">
			{#if remote?.web_url !== undefined}
				{@const mark = RepositoryMarkFor(remote.host)}
				{@const MarkIcon = mark.icon}
				<MarkIcon class={RepositoryMarkClass(mark, "size-3.5")} />
				<!-- no-drag frees the link from the desktop titlebar's drag region; inert on the web. -->
				<a
					href={remote.web_url}
					target="_blank"
					rel="noreferrer"
					class="truncate text-(--banner-info) underline-offset-2 transition-colors duration-(--duration-fast) ease-in-out [-webkit-app-region:no-drag] hover:underline motion-reduce:transition-none"
				>
					{RepositoryQualifiedLabel(remote.web_url)}
				</a>
			{:else}
				<!-- A repository with no web remote is the folder it lives in. -->
				<Folder class="size-3.5 shrink-0" />
				<span class="truncate">{project.display_name}</span>
			{/if}
			<span class="shrink-0">on</span>
			<GitBranch class="size-3.5 shrink-0" />
			<span class="truncate">
				{repository.branch.type === "detached" ? "detached HEAD" : repository.branch.name}
			</span>
		</span>
	{:else}
		<span class="flex min-w-0 items-center gap-1.5 text-sm text-muted-foreground">
			<Folder class="size-3.5 shrink-0" />
			<span class="truncate">{project.display_name}</span>
		</span>
	{/if}
{/if}

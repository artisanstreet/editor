<script lang="ts" effect>
	import GitBranch from "@tabler/icons-svelte/icons/git-branch";
	import { Effect } from "effect";
	import { RunBrowserDom } from "$lib/browser/dom";
	import type { Project, ProjectRepository } from "@artisan/protocol";
	import { ShortProjectPath } from "$lib/root/project-path";
	import { RepositoryLinkLabel } from "$lib/vcs/labels";
	import { RepositoryMarkClass, RepositoryMarkFor } from "$lib/vcs/presentation";

	let {
		project,
		repository,
	}: {
		project: Project;
		repository: ProjectRepository | undefined;
	} = $props();

	const StopPropagation = (event: MouseEvent) =>
		Effect.gen(function* () {
			yield* RunBrowserDom(() => event.stopPropagation());
		});
</script>

{#if repository === undefined || repository.state !== "repository"}
	<span class="truncate text-sm text-muted-foreground">
		{ShortProjectPath(project.root_path, project.display_name) ?? project.root_path}
	</span>
{:else}
	{@const remote = repository.remotes.find((candidate) => candidate.name === repository.default_remote)}
	{@const mark = RepositoryMarkFor(remote?.host)}
	{@const MarkIcon = mark.icon}
	<span class="flex min-w-0 items-center gap-1.5 text-sm text-muted-foreground">
		<MarkIcon class={RepositoryMarkClass(mark, "size-3.5")} />
		{#if remote?.web_url !== undefined}
			<a
				href={remote.web_url}
				target="_blank"
				rel="noreferrer"
				class="truncate text-(--banner-info) underline-offset-2 transition-colors duration-(--duration-fast) ease-in-out hover:underline motion-reduce:transition-none"
				onclick={yield* StopPropagation(event)}
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

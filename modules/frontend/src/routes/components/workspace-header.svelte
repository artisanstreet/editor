<script lang="ts" effect>
	import Folder from "@tabler/icons-svelte/icons/folder";
	import FolderCode from "@tabler/icons-svelte/icons/folder-code";
	import GitBranch from "@tabler/icons-svelte/icons/git-branch";
	import { Effect, Stream } from "effect";
	import type { Project, ProjectRepository } from "@artisan/protocol";
	import { ProjectRepositoryController } from "$lib/workspace/project-repository-controller";
	import { RepositoryLinkLabel, RepositoryQualifiedLabel } from "$lib/vcs/labels";
	import { RepositoryMarkClass, RepositoryMarkFor } from "$lib/vcs/presentation";

	let {
		project,
		thread_title,
	}: {
		/** The workspace the open route is inside; the header renders nothing without one. */
		project: Project | undefined;
		/** The open thread's own name, closing the line as its foreground subject. */
		thread_title?: string;
	} = $props();

	/**
	 * This retained projection is shared with the thread environment card. The
	 * header asks only for the project it is about to display; it never turns a
	 * single active workspace into Git inspection for every attached project.
	 */
	const repository_controller = yield* ProjectRepositoryController;
	let repositories = $state.raw<ReadonlyMap<string, ProjectRepository | undefined>>(
		yield* repository_controller.Current,
	);
	const ApplyRepositories = (
		next: ReadonlyMap<string, ProjectRepository | undefined>,
	) =>
		Effect.sync(() => {
			repositories = next;
		});
	yield* repository_controller.Changes.pipe(
		Stream.runForEach(ApplyRepositories),
		Effect.forkScoped,
	);
	/** Paint the retained projection first; inspection must never hold up the titlebar. */
	yield* repository_controller.Refresh(project?.project_id).pipe(Effect.forkScoped);

	const repository = $derived(
		project === undefined ? undefined : repositories.get(project.project_id),
	);
</script>

{#if project !== undefined}
	<!-- Icons carry translate-y-px: box-centering leaves their ink a pixel above the
	     lowercase words beside them, so the glyphs are aligned rather than the boxes. -->
	<span class="flex min-w-0 items-center gap-1.5 text-sm text-muted-foreground">
		{#if repository !== undefined && repository.state === "repository"}
			{@const remote = repository.remotes.find(
				(candidate) => candidate.name === repository.default_remote,
			)}
			{#if remote?.web_url !== undefined}
				{@const mark = RepositoryMarkFor(remote.host)}
				{@const MarkIcon = mark.icon}
				<MarkIcon class={RepositoryMarkClass(mark, "size-3.5 translate-y-px")} />
				<!-- no-drag frees the link from the desktop titlebar's drag region; inert on the web. -->
				<a
					href={remote.web_url}
					target="_blank"
					rel="noreferrer"
					class="shrink-0 text-(--banner-info) underline-offset-2 transition-colors duration-(--duration-fast) ease-in-out [-webkit-app-region:no-drag] hover:underline motion-reduce:transition-none"
				>
					{RepositoryQualifiedLabel(remote.web_url)}
				</a>
			{:else}
				<!-- A repository with no web remote is the folder it lives in. -->
				<Folder class="size-3.5 shrink-0 translate-y-px" />
				<span class="shrink-0">{project.display_name}</span>
			{/if}
			<span class="shrink-0">on</span>
			<GitBranch class="size-3.5 shrink-0 translate-y-px" />
			<span class="shrink-0">
				{repository.branch.type === "detached" ? "detached HEAD" : repository.branch.name}
			</span>
			{#if remote?.web_url !== undefined &&
				project.display_name.toLowerCase() !==
					RepositoryLinkLabel(remote.web_url).toLowerCase() &&
				project.display_name.toLowerCase() !== "default"}
				<!-- The remote names the repository; this names the checkout working it. A
				     checkout named after the repository itself — or the default worktree —
				     restates the remote label, so only a diverging name earns the segment. -->
				<span class="shrink-0">in</span>
				<FolderCode class="size-3.5 shrink-0 translate-y-px" />
				<span class="shrink-0">{project.display_name}</span>
			{/if}
		{:else}
			<Folder class="size-3.5 shrink-0 translate-y-px" />
			<span class="shrink-0">{project.display_name}</span>
		{/if}
		{#if thread_title !== undefined}
			<!-- The line reads workspace, then subject: everything before the slash is fixed context;
			     the thread title owns the remaining width and is the only segment allowed to ellipsize. -->
			<span class="shrink-0">/</span>
			<span class="min-w-0 flex-1 truncate text-foreground">{thread_title}</span>
		{/if}
	</span>
{/if}

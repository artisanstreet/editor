<script lang="ts" effect>
	import DeviceLaptop from "@tabler/icons-svelte/icons/device-laptop";
	import FileDiff from "@tabler/icons-svelte/icons/file-diff";
	import FolderCode from "@tabler/icons-svelte/icons/folder-code";
	import GitBranch from "@tabler/icons-svelte/icons/git-branch";
	import Selector from "@tabler/icons-svelte/icons/selector";
	import type {
		GitBranchState,
		GitRepositoryProjection,
		HostIdentitySnapshot,
		HostMachineSnapshot,
		HostMachinesSnapshot,
		Project,
		ProjectRepository,
	} from "@artisan/protocol";
	import { Effect, Stream } from "effect";
	import { FormatPathSeparators } from "$lib/appearance/display-format";
	import { path_separator } from "$lib/appearance-config";
	import {
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuTrigger,
	} from "$lib/components/ui/dropdown-menu";
	import { RepositoryDestinationLabel } from "$lib/vcs/labels";
	import { RepositoryChipMarkClass, RepositoryMarkFor } from "$lib/vcs/presentation";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { RuntimeSurfaceFor } from "$lib/browser/runtime-surface";
	import { HostIdentityController } from "$lib/identity/host-identity-controller";
	import { HostMachinesController } from "$lib/identity/host-machines-controller";
	import {
		build_machine_switch_url,
		RecallHomeHost,
		RememberHomeHost,
	} from "$lib/identity/machine-switch";
	import { RequestForgeRepair } from "$lib/root/forge-repair-request.svelte";
	import type { RecentProject } from "$lib/root/project-catalog";
	import { ProjectRepositoryController } from "$lib/workspace/project-repository-controller";
	import {
		GitWorkspaceController,
		GitWorkspaceKey,
		type GitWorkspaceState,
	} from "$lib/workspace/git-workspace-controller";
	import HoverPill, { type PillHover } from "./hover-pill.svelte";
	import ProjectSelector from "./project-selector.svelte";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";

	let {
		hover,
		onnewproject,
		onselectproject,
		project,
		project_id,
		project_root_path,
		projects,
		thread_id,
		workspace_id,
	}: {
		readonly hover: PillHover;
		readonly onnewproject: Effect.Effect<void>;
		readonly onselectproject: (project: Project) => Effect.Effect<void>;
		readonly project: Project | undefined;
		readonly project_id: string | undefined;
		readonly project_root_path: string | undefined;
		readonly projects: ReadonlyArray<RecentProject>;
		readonly thread_id: string | undefined;
		readonly workspace_id: string | undefined;
	} = $props();

	const identity_controller = yield* HostIdentityController;
	const machines_controller = yield* HostMachinesController;
	const repository_controller = yield* ProjectRepositoryController;
	const git_workspace_controller = yield* GitWorkspaceController;
	let identity = $state<HostIdentitySnapshot | undefined>(yield* identity_controller.Current);
	let machines = $state<HostMachinesSnapshot | undefined>(yield* machines_controller.Current);
	let switching = $state<string | undefined>(undefined);
	let switch_error = $state<{ readonly id: string; readonly message: string } | undefined>(
		undefined,
	);
	const desktop = yield* RunBrowserDom(
		() => RuntimeSurfaceFor(globalThis.navigator?.userAgent ?? "") === "desktop",
	);
	const home_host = yield* RecallHomeHost;
	let repositories = $state.raw<ReadonlyMap<string, ProjectRepository | undefined>>(
		yield* repository_controller.Current,
	);
	let git_workspaces = $state.raw<GitWorkspaceState>(yield* git_workspace_controller.Current);
	let workspace = $state<GitRepositoryProjection | undefined>(undefined);
	let request_generation = 0;

	const ApplyIdentity = (next: HostIdentitySnapshot | undefined) =>
		Effect.gen(function* () {
			identity = next;
		});
	yield* identity_controller.Changes.pipe(Stream.runForEach(ApplyIdentity), Effect.forkScoped);
	const ApplyMachines = (next: HostMachinesSnapshot | undefined) =>
		Effect.sync(() => {
			machines = next;
		});
	yield* machines_controller.Changes.pipe(Stream.runForEach(ApplyMachines), Effect.forkScoped);
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
	const ApplyGitWorkspaces = (next: GitWorkspaceState) =>
		Effect.sync(() => {
			git_workspaces = next;
			workspace =
				thread_id === undefined || workspace_id === undefined
					? undefined
					: next.get(GitWorkspaceKey({ thread_id, workspace_id }));
		});
	yield* git_workspace_controller.Changes.pipe(
		Stream.runForEach(ApplyGitWorkspaces),
		Effect.forkScoped,
	);

	const LoadEnvironment = (
		generation: number,
		next_project_id: string | undefined,
		next_thread_id: string | undefined,
		next_workspace_id: string | undefined,
	) =>
		Effect.gen(function* () {
			yield* Effect.all(
				[
					repository_controller.Refresh(next_project_id),
					next_thread_id === undefined || next_workspace_id === undefined
						? Effect.void
						: git_workspace_controller.Refresh({
								thread_id: next_thread_id,
								workspace_id: next_workspace_id,
							}),
				],
				{ concurrency: "unbounded" },
			);
			if (generation !== request_generation) return;
			workspace =
				next_thread_id === undefined || next_workspace_id === undefined
					? undefined
					: git_workspaces.get(
							GitWorkspaceKey({
								thread_id: next_thread_id,
								workspace_id: next_workspace_id,
							}),
						);
		});

	const RefreshEnvironment = (
		next_project_id: string | undefined,
		next_thread_id: string | undefined,
		next_workspace_id: string | undefined,
	) =>
		Effect.gen(function* () {
			const generation = ++request_generation;
			workspace =
				next_thread_id === undefined || next_workspace_id === undefined
					? undefined
					: git_workspaces.get(
							GitWorkspaceKey({
								thread_id: next_thread_id,
								workspace_id: next_workspace_id,
							}),
						);
			yield* LoadEnvironment(
				generation,
				next_project_id,
				next_thread_id,
				next_workspace_id,
			).pipe(Effect.forkScoped);
		});

	yield* identity_controller.Refresh.pipe(Effect.forkScoped);
	yield* machines_controller.Refresh.pipe(Effect.forkScoped);
	yield* RefreshEnvironment(project_id, thread_id, workspace_id);

	const repository = $derived(
		project_id === undefined ? undefined : repositories.get(project_id),
	);

	/**
	 * The connected Forge is always the snapshot's first machine, so it names
	 * the row ("This computer", or "This computer on WSL2" when Forge runs
	 * inside a distribution). The raw hostname survives only as the fallback
	 * while the machines query is in flight.
	 */
	const machine_choices = $derived(machines?.machines ?? []);
	const current_machine = $derived(machine_choices.at(0));
	const machine_label = $derived(
		current_machine?.label ?? identity?.hostname ?? "Not connected",
	);
	/**
	 * A WSL-hosted Forge is a peer the desktop shell started on request; the
	 * way back to the shell's own Forge is the shell's handoff, asked for over
	 * the repair channel. Only the desktop can answer that ask, so the return
	 * row exists only there.
	 */
	const on_peer_forge = $derived(current_machine?.label === "This computer on WSL2");
	const home_row = $derived(
		desktop && on_peer_forge ? (home_host ?? { label: "This computer" }) : undefined,
	);

	const SwitchToMachine = (machine: HostMachineSnapshot) =>
		Effect.gen(function* () {
			if (switching !== undefined || machine.kind !== "wsl") return;
			switching = machine.id;
			switch_error = undefined;
			const outcome = yield* machines_controller.Connect(machine.id);
			if (outcome === undefined || outcome.status === "failed") {
				switching = undefined;
				switch_error = {
					id: machine.id,
					message:
						outcome === undefined
							? "Forge did not answer the connect request."
							: outcome.message,
				};
				return;
			}
			yield* RememberHomeHost({
				...(identity?.hostname === undefined ? {} : { detail: identity.hostname }),
				label: current_machine?.label ?? "This computer",
			});
			yield* RunBrowserDom(() => {
				location.assign(
					build_machine_switch_url(
						location.protocol,
						location.origin,
						outcome.endpoint,
						outcome.pair_code,
						`machine-switch-${Date.now()}`,
					),
				);
			}).pipe(Effect.ignore);
		});

	const ReturnToHome = () =>
		Effect.gen(function* () {
			if (switching !== undefined) return;
			switching = "home";
			switch_error = undefined;
			yield* RequestForgeRepair;
		});

	const BranchLabel = (branch: GitBranchState | undefined): string => {
		if (branch === undefined) return "No branch";
		return branch.type === "detached" ? "Detached HEAD" : branch.name;
	};

	/**
	 * A worktree's only identity is its absolute path, but the row is a display
	 * surface: the directory name is what a person recognizes, so the path
	 * reduces to its last segment and the dropdown keeps the full path as a
	 * secondary line for disambiguation.
	 */
	const WorktreeLabel = (path: string): string =>
		path.split(/[\\/]+/u).filter((segment) => segment !== "").at(-1) ?? path;

	const current_worktree = $derived(workspace?.worktrees.find((candidate) => candidate.is_current));
	const current_worktree_path = $derived(current_worktree?.path ?? project_root_path);
	const worktree_paths = $derived(
		workspace?.worktrees.map((worktree) => worktree.path) ??
			(project_root_path === undefined ? [] : [project_root_path]),
	);
	const current_branch = $derived(
		workspace?.branch ?? (repository?.state === "repository" ? repository.branch : undefined),
	);
	const change_summary = $derived(workspace?.aggregate);
	const default_remote = $derived(
		repository?.state === "repository"
			? repository.remotes.find((candidate) => candidate.name === repository.default_remote)
			: undefined,
	);
	const branch_choices = $derived.by(() => {
		const choices = new Map<string, GitBranchState>();
		for (const worktree of workspace?.worktrees ?? []) {
			if (worktree.branch === undefined) continue;
			choices.set(BranchLabel(worktree.branch), worktree.branch);
		}
		if (current_branch !== undefined) choices.set(BranchLabel(current_branch), current_branch);
		return [...choices.values()];
	});
</script>

<section aria-label="Thread context">
	<ShaderGlassSurface
		class="radius-surface min-h-0 max-h-full shrink [--radius-gap:var(--spacing)] [--radius-surface:var(--radius-xl)]"
	>
		<div class="min-w-0 p-1">
			<div class="relative flex flex-col text-sm">
				<HoverPill {hover} />
				<ProjectSelector
					{hover}
					onnewproject={onnewproject}
					onselect={onselectproject}
					{project}
					{projects}
				/>
				<div class="flex min-w-0 flex-col">
				{#if machine_choices.length > 1 || home_row !== undefined}
					<DropdownMenu>
						<DropdownMenuTrigger
							class="relative flex min-w-0 items-center gap-2 rounded-lg px-2 py-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
							onpointerenter={hover.move}
							onpointermove={hover.move}
							onfocusin={hover.move}
						>
							<DeviceLaptop class="size-4 shrink-0 text-muted-foreground" />
							<span class="min-w-0 flex-1 text-left text-foreground">Machine</span>
							<span class="max-w-36 truncate text-foreground">{machine_label}</span>
							<Selector
								class="pointer-events-none size-3.5 shrink-0 text-muted-foreground"
								aria-hidden="true"
							/>
						</DropdownMenuTrigger>
						<DropdownMenuContent align="end" class="min-w-64 bg-transparent! p-0! shadow-none! ring-0!">
							<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
								<div class="flex flex-col p-1 text-sm" aria-label="Available machines">
									{#if home_row !== undefined}
										<button
											type="button"
											class="flex min-w-0 flex-col rounded-lg px-2 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:opacity-60"
											disabled={switching !== undefined}
											onclick={yield* ReturnToHome()}
										>
											<span class="truncate text-foreground">{home_row.label}</span>
											<span class="truncate text-xs text-muted-foreground">
												{switching === "home" ? "Returning…" : (home_row.detail ?? "Return to this desktop's Forge")}
											</span>
										</button>
									{/if}
									{#each machine_choices as machine (machine.id)}
										{#if machine.kind === "wsl"}
											<button
												type="button"
												class="flex min-w-0 flex-col rounded-lg px-2 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50 disabled:opacity-60"
												disabled={switching !== undefined}
												onclick={yield* SwitchToMachine(machine)}
											>
												<span class="truncate text-foreground">{machine.label}</span>
												<span
													class="truncate text-xs {switch_error?.id === machine.id
														? 'text-red-400'
														: 'text-muted-foreground'}"
												>
													{switching === machine.id
														? "Starting…"
														: (switch_error?.id === machine.id
																? switch_error.message
																: machine.detail)}
												</span>
											</button>
										{:else}
											<div class="flex min-w-0 flex-col rounded-lg px-2 py-2">
												<span class="truncate text-foreground">{machine.label}</span>
												{#if machine.detail !== undefined}
													<span class="truncate text-xs text-muted-foreground">{machine.detail}</span>
												{/if}
											</div>
										{/if}
									{/each}
								</div>
							</ShaderGlassSurface>
						</DropdownMenuContent>
					</DropdownMenu>
				{:else}
					<div class="flex min-w-0 items-center gap-2 rounded-lg px-2 py-2">
						<DeviceLaptop class="size-4 shrink-0 text-muted-foreground" />
						<span class="min-w-0 flex-1 text-foreground">Machine</span>
						<span class="max-w-36 truncate text-foreground">{machine_label}</span>
					</div>
				{/if}

				{#if change_summary !== undefined}
					<div class="flex min-w-0 items-center gap-2 rounded-lg px-2 py-2">
						<FileDiff class="size-4 shrink-0 text-muted-foreground" />
						<span class="min-w-0 flex-1 text-foreground">Changes</span>
						<span class="font-mono tabular-nums text-emerald-400">+{change_summary.lines_added}</span>
						<span class="font-mono tabular-nums text-red-400">−{change_summary.lines_deleted}</span>
					</div>
				{/if}

				{#if current_branch !== undefined}
					<DropdownMenu>
						<DropdownMenuTrigger
							class="relative flex min-w-0 items-center gap-2 rounded-lg px-2 py-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
							onpointerenter={hover.move}
							onpointermove={hover.move}
							onfocusin={hover.move}
						>
							<GitBranch class="size-4 shrink-0 text-muted-foreground" />
							<span class="min-w-0 flex-1 text-left text-foreground">Branch</span>
							<span class="max-w-36 truncate text-foreground">{BranchLabel(current_branch)}</span>
							<Selector
								class="pointer-events-none size-3.5 shrink-0 text-muted-foreground"
								aria-hidden="true"
							/>
						</DropdownMenuTrigger>
						<DropdownMenuContent align="end" class="min-w-56 bg-transparent! p-0! shadow-none! ring-0!">
							<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
								<div class="flex flex-col p-1 text-sm" aria-label="Observed branches">
									{#each branch_choices as branch (BranchLabel(branch))}
										<div class="truncate rounded-lg px-2 py-2 text-foreground">{BranchLabel(branch)}</div>
									{/each}
								</div>
							</ShaderGlassSurface>
						</DropdownMenuContent>
					</DropdownMenu>
				{/if}

				{#if current_worktree_path !== undefined}
					<DropdownMenu>
						<DropdownMenuTrigger
							class="relative flex min-w-0 items-center gap-2 rounded-lg px-2 py-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
							onpointerenter={hover.move}
							onpointermove={hover.move}
							onfocusin={hover.move}
						>
							<FolderCode class="size-4 shrink-0 text-muted-foreground" />
							<span class="min-w-0 flex-1 text-left text-foreground">Worktree</span>
							<span class="max-w-36 truncate text-foreground">{WorktreeLabel(current_worktree_path)}</span>
							<Selector
								class="pointer-events-none size-3.5 shrink-0 text-muted-foreground"
								aria-hidden="true"
							/>
						</DropdownMenuTrigger>
						<DropdownMenuContent align="end" class="min-w-72 bg-transparent! p-0! shadow-none! ring-0!">
							<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
								<div class="flex flex-col p-1 text-sm" aria-label="Observed worktrees">
									{#each worktree_paths as worktree_path (worktree_path)}
										<div class="flex min-w-0 flex-col rounded-lg px-2 py-2">
											<span class="truncate text-foreground">{WorktreeLabel(worktree_path)}</span>
											<span class="truncate text-xs text-muted-foreground"
												>{FormatPathSeparators(worktree_path, $path_separator)}</span
											>
										</div>
									{/each}
								</div>
							</ShaderGlassSurface>
						</DropdownMenuContent>
					</DropdownMenu>
				{/if}

				{#if default_remote?.web_url !== undefined}
					{@const mark = RepositoryMarkFor(default_remote.host)}
					{@const MarkIcon = mark.icon}
					<!-- The chip identifies the host without navigating; the destination
					     is no longer visible text, so it survives as the accessible name. -->
					<div
						class="card-plastic relative mt-2 flex min-w-0 items-center justify-center rounded-(--radius-nested) px-2 py-2 {mark.chip}"
						role="img"
						aria-label={RepositoryDestinationLabel(default_remote.web_url)}
					>
						<MarkIcon class={RepositoryChipMarkClass(mark, "size-4")} />
					</div>
				{/if}
				</div>
			</div>
		</div>
	</ShaderGlassSurface>
</section>

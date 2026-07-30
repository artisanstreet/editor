<script lang="ts">
	import { goto } from "$app/navigation";
	import Edit from "@tabler/icons-svelte/icons/edit";
	import MessageCircle from "@tabler/icons-svelte/icons/message-circle";
	import type { ThreadListItem } from "@artisan/protocol";
	import {
		CommandDialog,
		CommandEmpty,
		CommandGroup,
		CommandInput,
		CommandItem,
		CommandList,
	} from "$lib/components/ui/command";
	import {
		ProjectScopedThreadGroups,
		ThreadRoutePath,
	} from "$lib/root/thread-navigation";

	let {
		open = $bindable(false),
		threads,
	}: {
		open?: boolean;
		/** The live thread list, owned by the layout. */
		threads: ReadonlyArray<ThreadListItem>;
	} = $props();

	const project_thread_groups = $derived(ProjectScopedThreadGroups(threads));

	/** Selecting closes the dialog first so the destination never renders behind it. */
	const Navigate = (path: string) => {
		open = false;
		void goto(path);
	};
</script>

<svelte:window
	onkeydown={(event) => {
		if (event.key === "k" && (event.metaKey || event.ctrlKey)) {
			event.preventDefault();
			open = !open;
		}
	}}
/>

<CommandDialog bind:open title="Command menu" description="Search threads and actions">
	<CommandInput placeholder="Search threads and actions…" />
	<CommandList>
		<CommandEmpty>No results found.</CommandEmpty>

		<CommandGroup heading="Actions">
			<!--
				New thread is a plain jump into the bare draft route: no dropdown,
				no project picking, and no durable thread creation from the menu.
			-->
			<CommandItem onSelect={() => Navigate("/threads")}>
				<Edit />
				<span>New thread</span>
			</CommandItem>
		</CommandGroup>

		{#each project_thread_groups as group (group.type === "project" ? group.project.project_id : "unassigned")}
			{@const project = group.type === "project" ? group.project : undefined}
			<CommandGroup heading={project?.display_name ?? "Unassigned"}>
				{#each group.threads as thread (thread.thread_id)}
					<CommandItem
						value={`${thread.title} ${thread.thread_id}`}
						onSelect={() => Navigate(ThreadRoutePath(thread.thread_id))}
					>
						<MessageCircle />
						<span class="truncate">{thread.title}</span>
					</CommandItem>
				{/each}
			</CommandGroup>
		{/each}
	</CommandList>
</CommandDialog>

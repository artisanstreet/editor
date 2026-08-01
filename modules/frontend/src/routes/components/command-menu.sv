
<script lang="ts" effect>
	import Edit from "@tabler/icons-svelte/icons/edit";
	import MessageCircle from "@tabler/icons-svelte/icons/message-circle";
	import Settings from "@tabler/icons-svelte/icons/settings";
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
		ThreadRoutePathFor,
	} from "$lib/root/thread-navigation";
	import { Effect } from "effect";
	import { RunBrowserDom } from "$lib/browser/dom";

	let {
		open = $bindable(false),
		threads,
	}: {
		open?: boolean;
		/** The live thread list, owned by the layout. */
		threads: ReadonlyArray<ThreadListItem>;
	} = $props();

	const project_thread_groups = $derived(ProjectScopedThreadGroups(threads));

	const ToggleCommandMenu = (event: KeyboardEvent) =>
		Effect.gen(function* () {
			if (event.key !== "k" || (!event.metaKey && !event.ctrlKey)) return;
			yield* RunBrowserDom(() => event.preventDefault());
			open = !open;
		});

</script>

<svelte:window
	onkeydown={yield* ToggleCommandMenu(event)}
/>

<CommandDialog bind:open title="Command menu" description="Search threads and actions">
	<CommandInput placeholder="Search threads and actions…" />
	<CommandList>
		<CommandEmpty>No results found.</CommandEmpty>

		<CommandGroup heading="Actions">
			<!--
				New thread is a plain jump to the root draft: no dropdown,
				no project picking, and no durable thread creation from the menu.
			-->
			<CommandItem>
				<a href="/" class="flex grow items-center gap-2"><Edit /><span>New thread</span></a>
			</CommandItem>
			<CommandItem>
				<a href="/settings/models" class="flex grow items-center gap-2"><Settings /><span>Open settings</span></a>
			</CommandItem>
		</CommandGroup>

		{#each project_thread_groups as group (group.type === "project" ? group.project.project_id : "unassigned")}
			{@const project = group.type === "project" ? group.project : undefined}
			<CommandGroup heading={project?.display_name ?? "Unassigned"}>
				{#each group.threads as thread (thread.thread_id)}
					<CommandItem
						value={`${thread.title} ${thread.thread_id}`}
					>
						<a href={ThreadRoutePathFor(thread)} class="flex min-w-0 grow items-center gap-2"><MessageCircle /><span class="truncate">{thread.title}</span></a>
					</CommandItem>
				{/each}
			</CommandGroup>
		{/each}
	</CommandList>
</CommandDialog>


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
	import { RunBrowserDom } from "$lib/browser/dom";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import { thread_display_title, thread_title_mode } from "$lib/threads/title";
	import {
		PrepareNewThreadDraft,
		is_unmodified_primary_activation,
		new_thread_draft_key,
	} from "$lib/root/new-thread-draft";
	import {
		ProjectScopedThreadGroups,
		ThreadRoutePathFor,
	} from "$lib/root/thread-navigation";
	import { Effect } from "effect";

	let {
		open = $bindable(false),
		threads,
	}: {
		open?: boolean;
		/** The live thread list, owned by the layout. */
		threads: ReadonlyArray<ThreadListItem>;
	} = $props();

	const project_thread_groups = $derived(ProjectScopedThreadGroups(threads));
	const navigation = yield* RouteNavigation;

	const StartNewThread = (event: MouseEvent) =>
		Effect.gen(function* () {
			if (!is_unmodified_primary_activation(event)) return;
			yield* RunBrowserDom(() => event.preventDefault());
			/**
			 * A retained first message refuses the reset and keeps its recovery
			 * state — but the navigation is still the user's intent, and the new
			 * thread surface is where that retained message is explained and
			 * retried. Failing here instead made this action silently do nothing.
			 */
			yield* PrepareNewThreadDraft(new_thread_draft_key(undefined)).pipe(
				Effect.catchTag("DraftThreadLocked", () => Effect.void),
			);
			open = false;
			yield* navigation.Navigate("/");
		});

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
				<a href="/" class="flex grow items-center gap-2" onclick={yield* StartNewThread(event)}
					><Edit /><span>New thread</span></a
				>
			</CommandItem>
			<CommandItem>
				<a href="/settings/models" class="flex grow items-center gap-2"><Settings /><span>Open settings</span></a>
			</CommandItem>
		</CommandGroup>

		{#each project_thread_groups as group (group.type === "project" ? group.project.project_id : "unassigned")}
			{@const project = group.type === "project" ? group.project : undefined}
			<CommandGroup heading={project?.display_name ?? "Unassigned"}>
				{#each group.threads as thread (thread.thread_id)}
					{@const display_title = thread_display_title(thread, $thread_title_mode)}
					<!-- Both titles stay searchable: the reader remembers whichever they last saw. -->
					<CommandItem
						value={`${display_title} ${thread.title} ${thread.thread_id}`}
					>
						<a href={ThreadRoutePathFor(thread)} class="flex min-w-0 grow items-center gap-2"><MessageCircle /><span class="truncate">{display_title}</span></a>
					</CommandItem>
				{/each}
			</CommandGroup>
		{/each}
	</CommandList>
</CommandDialog>

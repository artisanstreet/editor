<script lang="ts" effect>
	import { Effect } from "effect";
	import { IconFileSearch as FileSearch } from "@tabler/icons-svelte";
	import type { WorkspaceFileReference } from "$lib/workspace/workspace-tab-model";
	import { Button } from "$lib/components/ui/button";
	import {
		Command,
		CommandEmpty,
		CommandGroup,
		CommandInput,
		CommandItem,
		CommandList,
	} from "$lib/components/ui/command";
	import {
		Dialog,
		DialogContent,
		DialogDescription,
		DialogHeader,
		DialogTitle,
	} from "$lib/components/ui/dialog";

	let {
		files,
		on_open,
	}: {
		files: ReadonlyArray<WorkspaceFileReference>;
		on_open: (file_id: string) => Effect.Effect<void>;
	} = $props();

	let is_open = $state(false);
	let query = $state("");
	let search_input = $state<HTMLInputElement>();

	const results = $derived.by(() => {
		const normalized = query.trim().toLocaleLowerCase();
		return files.filter(
			(file) =>
				normalized.length === 0 ||
				file.name.toLocaleLowerCase().includes(normalized) ||
				file.path.toLocaleLowerCase().includes(normalized),
		);
	});

	const OpenQuickOpen = Effect.gen(function* () {
		query = "";
		is_open = true;
		yield* Effect.sleep(0);
		search_input?.focus();
	});

	const CloseQuickOpen = Effect.gen(function* () {
		is_open = false;
	});

	const ChooseFile = (file_id: string) =>
		Effect.gen(function* () {
			yield* on_open(file_id);
			yield* CloseQuickOpen;
		});

	const HandleGlobalKeydown = (keyboard_event: KeyboardEvent) =>
		Effect.gen(function* () {
			if ((keyboard_event.ctrlKey || keyboard_event.metaKey) && keyboard_event.key.toLocaleLowerCase() === "p") {
				keyboard_event.preventDefault();
				if (!is_open) {
					yield* OpenQuickOpen;
				}
			}
		});
</script>

<svelte:window onkeydown={yield* HandleGlobalKeydown(event)} />

<Button variant="outline" size="xs" class="gap-2 text-muted-foreground" onclick={yield* OpenQuickOpen} aria-haspopup="dialog" aria-expanded={is_open}>
	<FileSearch size={14} stroke={1.7} aria-hidden="true" />
	<span class="hidden sm:inline">Quick open</span>
	<kbd class="hidden rounded border px-1 py-0.5 font-mono text-[0.65rem] sm:inline">Ctrl P</kbd>
</Button>

<Dialog bind:open={is_open}>
	<DialogContent class="max-w-xl gap-3 p-3" showCloseButton={false}>
		<DialogHeader class="sr-only">
			<DialogTitle>Quick open</DialogTitle>
			<DialogDescription>Search fixture files and open the selected file.</DialogDescription>
		</DialogHeader>
		<Command class="rounded-md" shouldFilter={false} loop>
			<CommandInput bind:ref={search_input} bind:value={query} placeholder="Search files by name or path" aria-label="Search fixture files" />
			<CommandList>
				<CommandEmpty>No fixture files match “{query}”.</CommandEmpty>
				<CommandGroup heading={`Fixture files (${results.length})`}>
					{#each results as file}
						<CommandItem
							value={file.path}
							onSelect={yield* ChooseFile(file.id)}
						>
							<div class="grid min-w-0 gap-0.5">
								<span>{file.name}</span>
								<span class="truncate text-xs text-muted-foreground">{file.path}</span>
							</div>
						</CommandItem>
					{/each}
				</CommandGroup>
			</CommandList>
		</Command>
	</DialogContent>
</Dialog>

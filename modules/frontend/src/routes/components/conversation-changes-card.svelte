<script lang="ts" effect>
	import type { ConversationItem } from "@artisan/protocol";
	import BrandVisualStudio from "@tabler/icons-svelte/icons/brand-visual-studio";
	import { Effect } from "effect";
	import { WriteClipboardText } from "$lib/browser/clipboard";
	import * as ContextMenu from "$lib/components/ui/context-menu";
	import { format_compact_diff_count } from "$lib/conversation/diff-stat";
	import {
		aggregate_file_change_diff,
		display_file_change_path,
		group_file_changes,
	} from "$lib/conversation/file-change-groups";
	import { resolve_file_icon } from "$lib/conversation/file-icon";
	import { changed_files_style_config } from "$lib/conversation-style-config";

	type ChangeSet = Extract<ConversationItem, { type: "change_set" }>;
	type FileChange = Extract<ConversationItem, { type: "file_change" }>;

	let {
		change_sets,
		files,
		project_root_path,
	}: {
		change_sets: ReadonlyArray<ChangeSet>;
		files: ReadonlyArray<FileChange>;
		project_root_path?: string;
	} = $props();

	const grouped_files = $derived(group_file_changes(files));
	const aggregate_diff = $derived(aggregate_file_change_diff(grouped_files));

	const file_count = $derived(
		grouped_files.length > 0
			? grouped_files.length
			: change_sets.reduce((count, change_set) => count + change_set.file_count, 0),
	);

	const path_parts = (path: string) => {
		const separator_index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));

		return separator_index === -1
			? { directory: "", filename: path }
			: {
					directory: path.slice(0, separator_index + 1),
					filename: path.slice(separator_index + 1),
				};
	};

	let copy_failed = $state(false);

	const CopyPath = (path: string) =>
		Effect.gen(function* () {
			copy_failed = false;
			yield* WriteClipboardText(path).pipe(
				Effect.catchTag("ClipboardWriteError", () =>
					Effect.gen(function* () {
						copy_failed = true;
					}),
				),
			);
		});
</script>

{#snippet diff_stat(additions, deletions, label)}
	<span
		role="group"
		aria-label={label}
		class="ml-auto inline-flex shrink-0 items-center justify-end gap-2 font-mono text-xs leading-4 tabular-nums"
	>
		<!--
			Content-sized, so the counts end flush with the row's right edge. A fixed
			numeric column reserved room for a width the count rarely uses, which left
			the pair floating short of the edge everything else in the card aligns to.
		-->
		<span aria-hidden="true" class="inline-grid grid-cols-[1ch_auto] gap-x-px text-green-500">
			<span>+</span>
			<span>{format_compact_diff_count(additions)}</span>
		</span>
		<span aria-hidden="true" class="inline-grid grid-cols-[1ch_auto] gap-x-px text-destructive">
			<span>-</span>
			<span>{format_compact_diff_count(deletions)}</span>
		</span>
	</span>
{/snippet}

<section
	class="flex w-full flex-col gap-1.5 overflow-hidden rounded-2xl bg-linear-to-t from-(--changed-files-from) to-(--changed-files-to) p-4"
	class:card={$changed_files_style_config.use_card}
	style:--changed-files-from={`var(--${$changed_files_style_config.from})`}
	style:--changed-files-to={`var(--${$changed_files_style_config.to})`}
>
	<header class="flex items-center justify-between gap-4">
		<p class="font-semibold">Edited {file_count} {file_count === 1 ? "file" : "files"}</p>
		{#if aggregate_diff.kind === "known"}
			{@render diff_stat(
				aggregate_diff.additions,
				aggregate_diff.deletions,
				`${aggregate_diff.additions} additions, ${aggregate_diff.deletions} deletions`,
			)}
		{/if}
	</header>

	{#if grouped_files.length > 0}
		<ul>
			{#each grouped_files as file (file.id)}
				{@const parts = path_parts(display_file_change_path(file.path, project_root_path))}
				<li>
					<ContextMenu.Root>
						<ContextMenu.Trigger
							data-operation={file.operation}
							class="file-row group/file-row flex w-full items-center justify-between gap-4 py-1.5 text-left"
						>
							<span class="flex min-w-0 items-center gap-2">
								<img
									src={resolve_file_icon(file.path)}
									alt=""
									aria-hidden="true"
									class="size-4 shrink-0"
								/>
								<span class="min-w-0 truncate text-sm">
									<span class="text-muted-foreground transition-colors duration-(--duration-quick) ease-in-out group-hover/file-row:text-foreground group-focus-visible/file-row:text-foreground"
										>{parts.directory}</span
									><span class="text-foreground">{parts.filename}</span>
								</span>
							</span>
							{#if file.diff.kind === "known"}
								{@render diff_stat(
									file.diff.additions,
									file.diff.deletions,
									`${file.diff.additions} additions, ${file.diff.deletions} deletions`,
								)}
							{/if}
						</ContextMenu.Trigger>

						<ContextMenu.Content class="w-64">
							<ContextMenu.Item>
								<BrandVisualStudio class="text-[#b67cff]" />
								Open in Visual Studio
							</ContextMenu.Item>
							<ContextMenu.Sub>
								<ContextMenu.SubTrigger inset>Open with</ContextMenu.SubTrigger>
								<ContextMenu.SubContent class="w-52">
									<ContextMenu.Item>Visual Studio</ContextMenu.Item>
									<ContextMenu.Item>Visual Studio Code</ContextMenu.Item>
									<ContextMenu.Item>System default</ContextMenu.Item>
								</ContextMenu.SubContent>
							</ContextMenu.Sub>
							<ContextMenu.Separator />
							<ContextMenu.Item onclick={yield* CopyPath(file.path)}>
								Copy path
							</ContextMenu.Item>
							<ContextMenu.Item>Copy file contents</ContextMenu.Item>
							<ContextMenu.Item>Open in Explorer</ContextMenu.Item>
						</ContextMenu.Content>
					</ContextMenu.Root>
				</li>
			{/each}
		</ul>
	{/if}
	{#if copy_failed}
		<p class="text-sm text-destructive" role="status">Couldn't copy the path. Try again.</p>
	{/if}
</section>

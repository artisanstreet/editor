<script lang="ts" effect>
	import type { ConversationItem } from "@artisan/protocol";
	import BrandVisualStudio from "@tabler/icons-svelte/icons/brand-visual-studio";
	import { Effect } from "effect";
	import { WriteClipboardText } from "$lib/browser/clipboard";
	import * as ContextMenu from "$lib/components/ui/context-menu";
	import { format_compact_diff_count } from "$lib/conversation/diff-stat";
	import { resolve_file_icon } from "$lib/conversation/file-icon";
	import { changed_files_style_config } from "$lib/conversation-style-config";

	type ChangeSet = Extract<ConversationItem, { type: "change_set" }>;
	type FileChange = Extract<ConversationItem, { type: "file_change" }>;

	let {
		change_sets,
		files,
	}: {
		change_sets: ReadonlyArray<ChangeSet>;
		files: ReadonlyArray<FileChange>;
	} = $props();

	const file_count = $derived(
		files.length > 0
			? files.length
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
		<span
			aria-hidden="true"
			class="inline-grid grid-cols-[1ch_4ch] gap-x-px text-left text-green-500"
		>
			<span>+</span>
			<span>{format_compact_diff_count(additions)}</span>
		</span>
		<span
			aria-hidden="true"
			class="inline-grid grid-cols-[1ch_4ch] gap-x-px text-left text-destructive"
		>
			<span>-</span>
			<span>{format_compact_diff_count(deletions)}</span>
		</span>
	</span>
{/snippet}

<section
	class="changed-files-card flex w-full flex-col gap-4 overflow-hidden rounded-2xl p-4"
	class:card={$changed_files_style_config.use_card}
	style:--changed-files-from={`var(--${$changed_files_style_config.from})`}
	style:--changed-files-to={`var(--${$changed_files_style_config.to})`}
>
	<p class="font-semibold">
		Edited {file_count} {file_count === 1 ? "file" : "files"}
	</p>

	{#if files.length > 0}
		<ul>
			{#each files as file (file.id)}
				{@const parts = path_parts(file.path)}
				<li>
					<ContextMenu.Root>
						<ContextMenu.Trigger
							class="group/file-row flex w-full items-center justify-between gap-4 py-1.5 text-left"
						>
							<span class="flex min-w-0 items-center gap-2">
								<img
									src={resolve_file_icon(file.path)}
									alt=""
									aria-hidden="true"
									class="size-4 shrink-0"
								/>
								<span class="min-w-0 truncate text-sm">
									<span class="file-row-directory text-muted-foreground"
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

<style>
	.changed-files-card {
		background-image: linear-gradient(
			to top,
			var(--changed-files-from),
			var(--changed-files-to)
		);
	}

	.file-row-directory {
		transition: color var(--duration-quick) var(--ease-in-out);
	}

	:global(.group\/file-row:hover) .file-row-directory,
	:global(.group\/file-row:focus-visible) .file-row-directory {
		color: var(--foreground);
	}

	@media (prefers-reduced-motion: reduce) {
		.file-row-directory {
			transition: none;
		}
	}
</style>

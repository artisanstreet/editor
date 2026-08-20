<script lang="ts" effect>
	import Folder from "@tabler/icons-svelte/icons/folder";
	import { Effect, Option } from "effect";
	import type { ProjectIdentitySource } from "@artisan/protocol";
	import {
		CreateBrowserObjectUrl,
		ReleaseBrowserObjectUrl,
	} from "$lib/browser/object-url";
	import { RichLinkAssetController } from "$lib/components/markdown/rich-link-asset-controller";
	import { RepositoryMarkClass, RepositoryMarkFor } from "$lib/vcs/presentation";

	let { identity }: { identity?: ProjectIdentitySource } = $props();

	const assets = yield* RichLinkAssetController;
	let image_source = $state(Option.none<string>());
	let owned_source: string | undefined;
	let generation = 0;

	const ReleaseImage = Effect.gen(function* () {
		const source = owned_source;
		owned_source = undefined;
		image_source = Option.none();
		if (source !== undefined) yield* ReleaseBrowserObjectUrl(source).pipe(Effect.ignore);
	});

	yield* Effect.addFinalizer(() => ReleaseImage);

	const RefreshImage = (next: ProjectIdentitySource | undefined) =>
		Effect.gen(function* () {
			const current_generation = (generation += 1);
			yield* ReleaseImage;
			if (next?.kind !== "repository" || next.image === undefined) return;

			const asset = yield* assets.Load(next.image).pipe(
				Effect.timeoutOption("2 seconds"),
				Effect.map(Option.getOrUndefined),
				Effect.catchCause(() => Effect.succeed(undefined)),
			);
			if (asset === undefined || current_generation !== generation || identity !== next) return;

			const source = yield* CreateBrowserObjectUrl(asset.bytes, asset.content_type).pipe(
				Effect.option,
				Effect.map(Option.getOrUndefined),
			);
			if (source === undefined) return;
			if (current_generation !== generation || identity !== next) {
				yield* ReleaseBrowserObjectUrl(source).pipe(Effect.ignore);
				return;
			}
			owned_source = source;
			image_source = Option.some(source);
		});

	yield* RefreshImage(identity).pipe(Effect.forkScoped);

	const ImageFailed = ReleaseImage;
	const repository_mark = $derived(
		identity?.kind === "repository" ? RepositoryMarkFor(identity.host) : undefined,
	);
</script>

<span
	aria-hidden="true"
	class="grid size-6 shrink-0 place-items-center overflow-hidden rounded-lg bg-surface-875 text-muted-foreground"
>
	{#if Option.isSome(image_source)}
		<img
			src={image_source.value}
			alt=""
			class="size-full object-cover"
			onerror={yield* ImageFailed}
		/>
	{:else if repository_mark !== undefined}
		{@const MarkIcon = repository_mark.icon}
		<MarkIcon class={RepositoryMarkClass(repository_mark, "size-3.5")} />
	{:else}
		<Folder class="size-3.5" />
	{/if}
</span>

<script lang="ts" effect>
	import { Dialog as DialogPrimitive } from "bits-ui";
	import X from "@tabler/icons-svelte/icons/x";
	import { Effect } from "effect";
	import { Button } from "$lib/components/ui/button";
	import { RunBrowserDom } from "$lib/browser/dom";

	let {
		open = $bindable(false),
		onclose,
		source,
		name,
	}: {
		open?: boolean;
		onclose?: () => Effect.Effect<void>;
		source?: string;
		name?: string;
	} = $props();

	let was_open = $state(open);
	const titlebar_overlay_height = yield* RunBrowserDom(() =>
		globalThis.navigator?.userAgent.includes("Electron/") ? "40px" : "0px",
	);

	const ReconcileOpenState = (is_open: boolean) =>
		Effect.gen(function* () {
			if (was_open && !is_open && onclose !== undefined) yield* onclose();
			was_open = is_open;
		});
	yield* ReconcileOpenState(open);

	const CloseBackdrop = (event: MouseEvent & { currentTarget: HTMLDivElement }) =>
		Effect.gen(function* () {
			if (event.currentTarget === event.target) open = false;
		});
</script>

<DialogPrimitive.Root bind:open>
	<DialogPrimitive.Portal>
		<DialogPrimitive.Overlay
			class="fixed inset-0 z-50 bg-black/70 supports-backdrop-filter:backdrop-blur-md"
		/>
		<DialogPrimitive.Content
			class="fixed inset-0 z-[51] flex size-full items-center justify-center p-8 pt-[calc(2rem+var(--titlebar-overlay-height,0px))] outline-none"
			style={`--titlebar-overlay-height: ${titlebar_overlay_height}`}
			aria-label={name === undefined ? "Image preview" : `Image preview: ${name}`}
			onclick={yield* CloseBackdrop(event)}
		>
			<DialogPrimitive.Title class="sr-only">
				{name === undefined ? "Image preview" : name}
			</DialogPrimitive.Title>
			{#if source !== undefined}
				<img
					src={source}
					alt={name ?? "Attached image"}
					class="h-auto w-auto max-h-full max-w-full object-contain"
					draggable="false"
				/>
			{/if}
			<DialogPrimitive.Close>
				{#snippet child({ props })}
					<Button
						{...props}
						variant="secondary"
						size="icon"
						class="absolute right-8 top-[calc(2rem+var(--titlebar-overlay-height,0px))]"
						aria-label="Close image preview"
					>
						<X />
					</Button>
				{/snippet}
			</DialogPrimitive.Close>
		</DialogPrimitive.Content>
	</DialogPrimitive.Portal>
</DialogPrimitive.Root>

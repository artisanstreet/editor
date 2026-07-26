<script lang="ts">
	import { Dialog as DialogPrimitive } from "bits-ui";
	import X from "@tabler/icons-svelte/icons/x";
	import { Button } from "$lib/components/ui/button";
	import { onMount } from "svelte";

	let {
		open = $bindable(false),
		onclose,
		source,
		name,
	}: {
		open?: boolean;
		onclose?: () => void;
		source?: string;
		name?: string;
	} = $props();

	let was_open = $state(open);
	let titlebar_overlay_height = $state("0px");

	onMount(() => {
		if (navigator.userAgent.includes("Electron/")) titlebar_overlay_height = "40px";
	});

	$effect(() => {
		if (was_open && !open) onclose?.();
		was_open = open;
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
			onclick={(event) => {
				if (event.currentTarget === event.target) open = false;
			}}
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

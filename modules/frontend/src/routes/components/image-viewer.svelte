<script lang="ts" effect>
	import { Dialog as DialogPrimitive } from "bits-ui";
	import X from "@tabler/icons-svelte/icons/x";
	import { Effect } from "effect";
	import { Button } from "$lib/components/ui/button";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { ImageInspectionStore } from "$lib/images/inspection-store";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";

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
	const inspection = yield* ImageInspectionStore;
	const titlebar_overlay_height = yield* RunBrowserDom(() =>
		globalThis.navigator?.userAgent.includes("Electron/") ? "40px" : "0px",
	);

	/**
	 * Surfaces that must stand down while an image is inspected — the proximity
	 * hover rail above all — are siblings of whatever opened this viewer, so the
	 * state travels through the store rather than a prop.
	 */
	const ReconcileOpenState = (is_open: boolean) =>
		Effect.gen(function* () {
			if (was_open === is_open) return;
			if (is_open) yield* inspection.Retain;
			else {
				yield* inspection.Release;
				if (onclose !== undefined) yield* onclose();
			}
			was_open = is_open;
		});
	yield* ReconcileOpenState(open);
	yield* Effect.addFinalizer(() =>
		Effect.gen(function* () {
			if (was_open) yield* inspection.Release;
		}),
	);

	/**
	 * The dialog fills the viewport, so there is no "outside" for the primitive's
	 * own dismissal to fire on, and its content does not forward a click handler
	 * to the DOM. This owns the gesture instead: a dismiss layer covering
	 * everything, with the image and the close button painted above it.
	 *
	 * Deliberately a plain handler — closing is state, not an effect, and a
	 * deferred fiber would be answering a click the user has already moved on
	 * from.
	 */
	const DismissViewer = () => {
		open = false;
	};
</script>

<DialogPrimitive.Root bind:open>
	<DialogPrimitive.Portal>
		<DialogPrimitive.Overlay
			class="fixed inset-0 z-50 bg-surface-1000/70 supports-backdrop-filter:backdrop-blur-md"
		/>
		<DialogPrimitive.Content
			class="fixed inset-0 z-[51] flex size-full items-center justify-center p-8 pt-[calc(2rem+var(--titlebar-overlay-height,0px))] outline-none"
			style={`--titlebar-overlay-height: ${titlebar_overlay_height}`}
			aria-label={name === undefined ? "Image preview" : `Image preview: ${name}`}
		>
			<DialogPrimitive.Title class="sr-only">
				{name === undefined ? "Image preview" : name}
			</DialogPrimitive.Title>
			<button
				type="button"
				class="absolute inset-0 cursor-default"
				aria-label="Close image preview"
				tabindex="-1"
				onclick={DismissViewer}
			></button>
			{#if source !== undefined}
				<div class="group/image relative z-10 inline-flex max-h-full max-w-full">
					<img
						src={source}
						alt={name ?? "Attached image"}
						class="h-auto w-auto max-h-full max-w-full object-contain"
						draggable="false"
					/>
					<DialogPrimitive.Close>
						{#snippet child({ props })}
							<ShaderGlassSurface
								class="pointer-events-none absolute right-2 top-2 size-8 rounded-full opacity-0 shadow-lg transition-opacity duration-150 group-hover/image:pointer-events-auto group-hover/image:opacity-100 focus-within:pointer-events-auto focus-within:opacity-100 motion-reduce:transition-none"
							>
								<Button
									{...props}
									variant="ghost"
									size="icon-sm"
									class="size-full rounded-full bg-transparent text-muted-foreground hover:bg-transparent hover:text-foreground"
									aria-label="Close image preview"
								>
									<X class="size-4" aria-hidden="true" />
								</Button>
							</ShaderGlassSurface>
						{/snippet}
					</DialogPrimitive.Close>
				</div>
			{/if}
		</DialogPrimitive.Content>
	</DialogPrimitive.Portal>
</DialogPrimitive.Root>

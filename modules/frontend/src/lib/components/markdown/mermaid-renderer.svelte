<script lang="ts" effect>
	import Maximize from "@tabler/icons-svelte/icons/maximize";
	import X from "@tabler/icons-svelte/icons/x";
	import ZoomIn from "@tabler/icons-svelte/icons/zoom-in";
	import ZoomOut from "@tabler/icons-svelte/icons/zoom-out";
	import { Data, Effect } from "effect";

	class MermaidRendererLoadFailure extends Data.TaggedError("MermaidRendererLoadFailure")<{
		readonly cause: unknown;
	}> {}

	let { content }: { content: string } = $props();
	const { render_conversation_mermaid } = yield* Effect.tryPromise({
		catch: (cause) => new MermaidRendererLoadFailure({ cause }),
		try: () => import("./mermaid-rendering"),
	});
	const rendered = $derived(render_conversation_mermaid(content));

	/**
	 * Diagrams are laid out at prose width, which routinely renders their text
	 * below legibility. The zoom overlay shows the same validated SVG at fit
	 * scale with wheel zoom around the pointer and drag panning.
	 */
	const zoom_minimum_scale = 0.25;
	const zoom_maximum_scale = 8;
	let zoom_open = $state(false);
	let zoom_scale = $state(1);
	let zoom_x = $state(0);
	let zoom_y = $state(0);
	let zoom_viewport = $state<HTMLDivElement>();
	let zoom_stage: HTMLDivElement | undefined;
	let panning = false;
	let pan_pointer = 0;
	let pan_last_x = 0;
	let pan_last_y = 0;
	let pan_travel = 0;

	const clamp_zoom_scale = (value: number): number =>
		Math.min(zoom_maximum_scale, Math.max(zoom_minimum_scale, value));

	const FitZoom = () => {
		if (zoom_viewport === undefined || zoom_stage === undefined) return;
		const bounds = zoom_stage.getBoundingClientRect();
		const natural_width = bounds.width / zoom_scale;
		const natural_height = bounds.height / zoom_scale;
		if (natural_width <= 0 || natural_height <= 0) return;
		zoom_scale = clamp_zoom_scale(
			Math.min(
				(zoom_viewport.clientWidth * 0.9) / natural_width,
				(zoom_viewport.clientHeight * 0.9) / natural_height,
			),
		);
		zoom_x = 0;
		zoom_y = 0;
	};

	/** Runs on overlay mount, after the stage has laid out at scale 1. */
	const InitializeZoomStage = (stage: HTMLDivElement) => {
		zoom_stage = stage;
		FitZoom();
	};

	const OpenZoom = () => {
		zoom_scale = 1;
		zoom_x = 0;
		zoom_y = 0;
		zoom_open = true;
	};
	const CloseZoom = () => {
		zoom_open = false;
	};

	const AdjustZoom = (factor: number, origin_x?: number, origin_y?: number) => {
		if (zoom_viewport === undefined) return;
		const next = clamp_zoom_scale(zoom_scale * factor);
		const applied = next / zoom_scale;
		if (origin_x !== undefined && origin_y !== undefined) {
			const bounds = zoom_viewport.getBoundingClientRect();
			const center_x = bounds.left + bounds.width / 2 + zoom_x;
			const center_y = bounds.top + bounds.height / 2 + zoom_y;
			zoom_x += (center_x - origin_x) * (applied - 1);
			zoom_y += (center_y - origin_y) * (applied - 1);
		}
		zoom_scale = next;
	};

	const OnZoomWheel = (event: WheelEvent) => {
		event.preventDefault();
		AdjustZoom(Math.exp(-event.deltaY * 0.0015), event.clientX, event.clientY);
	};

	const OnZoomPointerDown = (event: PointerEvent) => {
		if (event.button !== 0) return;
		panning = true;
		pan_pointer = event.pointerId;
		pan_last_x = event.clientX;
		pan_last_y = event.clientY;
		pan_travel = 0;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
	};
	const OnZoomPointerMove = (event: PointerEvent) => {
		if (!panning || event.pointerId !== pan_pointer) return;
		zoom_x += event.clientX - pan_last_x;
		zoom_y += event.clientY - pan_last_y;
		pan_travel += Math.abs(event.clientX - pan_last_x) + Math.abs(event.clientY - pan_last_y);
		pan_last_x = event.clientX;
		pan_last_y = event.clientY;
	};
	const OnZoomPointerUp = (event: PointerEvent) => {
		if (event.pointerId === pan_pointer) panning = false;
	};

	/** A click on the dimmed backdrop closes; a pan that ended there does not. */
	const OnZoomViewportClick = (event: MouseEvent) => {
		if (pan_travel > 4) return;
		if (event.target === event.currentTarget) CloseZoom();
	};
</script>

<svelte:window
	onkeydown={(event) => {
		if (zoom_open && event.key === "Escape") CloseZoom();
	}}
/>

{#if rendered.status === "rendered"}
	<div class="docs-mermaid-diagram not-prose" data-render-status="rendered">
		<button
			class="docs-mermaid-zoom-trigger"
			type="button"
			aria-label="Mermaid diagram — open zoom view"
			onclick={OpenZoom}
		>
			<!-- The adapter accepts only passive, structurally validated SVG. -->
			{@html rendered.html}
		</button>
	</div>
	{#if zoom_open}
		<div class="docs-mermaid-zoom-overlay" role="dialog" aria-modal="true" aria-label="Mermaid diagram">
			<div class="docs-mermaid-zoom-controls">
				<button
					class="docs-mermaid-zoom-control"
					type="button"
					aria-label="Zoom out"
					onclick={() => AdjustZoom(1 / 1.4)}
				>
					<ZoomOut class="size-4" />
				</button>
				<button
					class="docs-mermaid-zoom-control"
					type="button"
					aria-label="Zoom in"
					onclick={() => AdjustZoom(1.4)}
				>
					<ZoomIn class="size-4" />
				</button>
				<button
					class="docs-mermaid-zoom-control"
					type="button"
					aria-label="Fit diagram to screen"
					onclick={FitZoom}
				>
					<Maximize class="size-4" />
				</button>
				<button
					class="docs-mermaid-zoom-control"
					type="button"
					aria-label="Close zoom view"
					onclick={CloseZoom}
				>
					<X class="size-4" />
				</button>
			</div>
			<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
			<div
				class="docs-mermaid-zoom-viewport"
				bind:this={zoom_viewport}
				onwheel={OnZoomWheel}
				onpointerdown={OnZoomPointerDown}
				onpointermove={OnZoomPointerMove}
				onpointerup={OnZoomPointerUp}
				onpointercancel={OnZoomPointerUp}
				onclick={OnZoomViewportClick}
				ondblclick={FitZoom}
			>
				<div
					class="docs-mermaid-zoom-stage"
					use:InitializeZoomStage
					style={`transform: translate(${zoom_x}px, ${zoom_y}px) scale(${zoom_scale});`}
				>
					<!-- The adapter accepts only passive, structurally validated SVG. -->
					{@html rendered.html}
				</div>
			</div>
		</div>
	{/if}
{:else}
	<div class="docs-mermaid-diagram not-prose" data-render-status="invalid">
		<div class="docs-mermaid-error" role="note">
			<span>Unable to render this Mermaid diagram.</span>
			<pre><code>{content}</code></pre>
		</div>
	</div>
{/if}

<script lang="ts" effect>
	import { Effect } from "effect";
	import { RunBrowserDom } from "$lib/browser/dom";

	let {
		active_thread_id,
		layout_key,
		surface,
	}: {
		active_thread_id: string | undefined;
		/** Changes when rows move without a route change, prompting a fresh measurement. */
		layout_key: string;
		surface: HTMLElement | null;
	} = $props();

	let indicator_animated = $state(false);
	let indicator_height = $state(0);
	let indicator_top = $state(0);
	let indicator_visible = $state(false);
	let resize_revision = $state(0);

	/** Where the light was last placed, so only a genuine thread change travels. */
	let lit_thread_id: string | undefined = undefined;

	/**
	 * The light belongs to the list's scroll coordinates, not the viewport. A
	 * row therefore carries it naturally while scrolling, and only route, row
	 * order, or resize changes need another measurement.
	 */
	const PositionIndicator = (
		thread_id: string | undefined,
		current_surface: HTMLElement | null,
		_layout_key: string,
		_resize_revision: number,
	) =>
		Effect.gen(function* () {
			/** Let Svelte commit the new aria-current row before reading geometry. */
			yield* Effect.sleep("1 millis");
			const active_row = yield* RunBrowserDom(() =>
				current_surface?.querySelector<HTMLElement>('[aria-current="page"]'),
			);
			if (active_row === undefined || active_row === null || current_surface === null) {
				indicator_visible = false;
				return;
			}

			const { row_rect, scroll_top, surface_rect } = yield* RunBrowserDom(() => ({
				row_rect: active_row.getBoundingClientRect(),
				scroll_top: current_surface.scrollTop,
				surface_rect: current_surface.getBoundingClientRect(),
			}));
			/** First paint and remeasurement stay still; only another active row slides. */
			indicator_animated = indicator_visible && lit_thread_id !== thread_id;
			lit_thread_id = thread_id;
			indicator_height = row_rect.height;
			indicator_top = row_rect.top - surface_rect.top + scroll_top;
			indicator_visible = true;
		});

	const Resize = Effect.gen(function* () {
		resize_revision += 1;
	});

	/** SER owns the scoped geometry program and interrupts stale measurements. */
	yield* PositionIndicator(active_thread_id, surface, layout_key, resize_revision);
</script>

<svelte:window onresize={yield* Resize} />

<div
	class="thread-light"
	data-active={indicator_visible}
	data-animate={indicator_animated}
	aria-hidden="true"
	style={`--thread-light-y: ${indicator_top}px; --thread-light-height: ${indicator_height}px;`}
></div>

<style>
	/**
	 * The model picker's selected-provider lamp turned exactly sideways: its
	 * 2rem × 1.5rem beam becomes 1.5rem × 2rem, the gradient travels right, and
	 * the elliptical mask swaps axes and origin. Motion uses the same tokens.
	 */
	.thread-light {
		position: absolute;
		top: 0;
		left: 0;
		z-index: 1;
		width: 0;
		height: var(--thread-light-height, 0);
		pointer-events: none;
		opacity: 0;
		transform: translate3d(0, var(--thread-light-y, 0), 0);
		will-change: transform, height;
	}

	.thread-light::after {
		position: absolute;
		top: 50%;
		left: -0.125rem;
		width: 1.5rem;
		height: 2rem;
		content: "";
		background: linear-gradient(
			to right,
			oklch(from var(--foreground) l c h / 32%) 0%,
			oklch(from var(--foreground) l c h / 10%) 26%,
			oklch(from var(--foreground) l c h / 2%) 52%,
			transparent 74%
		);
		filter: blur(0.125rem);
		-webkit-mask-image: radial-gradient(
			ellipse 70% 48% at 35% 50%,
			#000 0%,
			rgb(0 0 0 / 50%) 42%,
			rgb(0 0 0 / 10%) 68%,
			transparent 88%
		);
		mask-image: radial-gradient(
			ellipse 70% 48% at 35% 50%,
			#000 0%,
			rgb(0 0 0 / 50%) 42%,
			rgb(0 0 0 / 10%) 68%,
			transparent 88%
		);
		transform: translateY(-50%);
	}

	.thread-light[data-active="true"] {
		opacity: 1;
	}

	.thread-light[data-animate="true"] {
		transition:
			transform var(--duration-fast) var(--ease-smooth-out),
			height var(--duration-fast) var(--ease-smooth-out);
	}

	@media (prefers-reduced-motion: reduce) {
		.thread-light {
			transition: none !important;
		}
	}
</style>

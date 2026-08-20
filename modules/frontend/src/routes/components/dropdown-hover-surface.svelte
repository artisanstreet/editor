<script lang="ts">
	import type { Snippet } from "svelte";

	let {
		children,
		class: class_name = "",
		flat = false,
		hold = false,
	}: {
		children: Snippet<[{ move_hover: (event: Event) => void }]>;
		class?: string;
		/**
		 * Skips the content wrapper's stacking context so the pill can travel
		 * across sibling card housings: static housing backgrounds then paint
		 * under the pill while positioned controls paint above it. Flat
		 * consumers must give their hoverable controls `position: relative`
		 * and keep housings free of transforms and filters at rest.
		 */
		flat?: boolean;
		/**
		 * Keeps the pill on the last hovered control after the pointer or focus
		 * leaves the surface, for pickers whose rows drive a sibling panel: the
		 * pill then marks the row that panel describes, and clearing it on the
		 * way over would orphan the panel. The pill still clears when its row
		 * leaves the document.
		 */
		hold?: boolean;
	} = $props();

	let surface = $state<HTMLElement>();
	let animated = $state(false);
	let active_target: HTMLElement | undefined;
	let height = $state(0);
	let has_seen_focus = false;
	let left = $state(0);
	let top = $state(0);
	let visible = $state(false);
	let width = $state(0);

	const clear_hover = () => {
		active_target = undefined;
		animated = false;
		visible = false;
	};

	/**
	 * Layout offsets rather than client rects: a dropdown measured while its
	 * content is still playing the zoom-and-slide entry animation would bake
	 * that transform into the pill and then visibly slide itself straight.
	 * offsetLeft/offsetTop ignore transforms, so the first read is already final.
	 */
	const offset_within_surface = (target: HTMLElement) => {
		let left_offset = 0;
		let node: HTMLElement | null = target;
		let top_offset = 0;

		while (node !== null && node !== surface) {
			left_offset += node.offsetLeft;
			top_offset += node.offsetTop;
			node = node.offsetParent instanceof HTMLElement ? node.offsetParent : null;
		}

		return { left: left_offset, top: top_offset };
	};

	const apply_hover = (target: HTMLElement) => {
		if (!surface?.contains(target)) {
			clear_hover();
			return;
		}
		const offset = offset_within_surface(target);

		active_target = target;
		animated = visible;
		height = target.offsetHeight;
		left = offset.left;
		top = offset.top;
		visible = true;
		width = target.offsetWidth;
	};

	/**
	 * Must be attached as a direct handler, never through an effectful (SER)
	 * one: it reads `currentTarget`, which exists only while the event is
	 * dispatching, and an effectful handler runs after dispatch has finished.
	 */
	const move_hover = (event: Event) => {
		if (!(event.currentTarget instanceof HTMLElement) || !surface) return;
		if (event.type === "focus") {
			if (!has_seen_focus) {
				has_seen_focus = true;
				return;
			}
		}

		apply_hover(event.currentTarget);
	};

	const clear_departed_focus = (event: FocusEvent) => {
		if (hold) return;
		if (event.relatedTarget instanceof Node && surface?.contains(event.relatedTarget)) return;
		clear_hover();
	};

	/**
	 * A keyed row can move or leave while the pointer remains inside this surface,
	 * which does not emit `pointerleave` on the surface itself. Reconcile the held
	 * target after dynamic-list mutations so its pill cannot remain at stale geometry.
	 */
	const observe_hover_target = (element: HTMLElement) => {
		const reconcile = () => {
			if (active_target === undefined) return;
			if (!element.contains(active_target)) {
				clear_hover();
				return;
			}
			if (!hold && !active_target.matches(":hover") && !active_target.matches(":focus-within")) {
				clear_hover();
				return;
			}
			apply_hover(active_target);
		};
		const mutation_observer = new MutationObserver(reconcile);
		const resize_observer = new ResizeObserver(reconcile);
		mutation_observer.observe(element, { childList: true, subtree: true });
		resize_observer.observe(element);

		return () => {
			mutation_observer.disconnect();
			resize_observer.disconnect();
		};
	};
</script>

<div
	bind:this={surface}
	class={`relative ${class_name}`}
	role="presentation"
	onpointerleave={hold ? undefined : clear_hover}
	onfocusout={clear_departed_focus}
	{@attach observe_hover_target}
>
	<div
		class="docs-sidebar-hover-highlight"
		data-active={visible}
		data-animate={animated}
		aria-hidden="true"
		style={`--docs-sidebar-hover-x: ${left}px; --docs-sidebar-hover-y: ${top}px; --docs-sidebar-hover-width: ${width}px; --docs-sidebar-hover-height: ${height}px;`}
	></div>
	<div class={flat ? "" : "relative z-1"}>
		{@render children({ move_hover })}
	</div>
</div>

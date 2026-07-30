<script lang="ts">
	import type { Snippet } from "svelte";

	let {
		children,
		class: class_name = "",
		flat = false,
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
	} = $props();

	let surface = $state<HTMLElement>();
	let animated = $state(false);
	let height = $state(0);
	let has_seen_focus = false;
	let left = $state(0);
	let top = $state(0);
	let visible = $state(false);
	let width = $state(0);

	const clear_hover = () => {
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

	const move_hover = (event: Event) => {
		if (!(event.currentTarget instanceof HTMLElement) || !surface) return;
		if (event.type === "focus") {
			if (!has_seen_focus) {
				has_seen_focus = true;
				return;
			}
		}

		const target = event.currentTarget;
		const offset = offset_within_surface(target);

		animated = visible;
		height = target.offsetHeight;
		left = offset.left;
		top = offset.top;
		visible = true;
		width = target.offsetWidth;
	};
</script>

<div
	bind:this={surface}
	class={`relative ${class_name}`}
	role="presentation"
	onpointerleave={clear_hover}
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

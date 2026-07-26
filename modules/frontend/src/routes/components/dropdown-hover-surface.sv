<script lang="ts">
	import type { Snippet } from "svelte";

	let {
		children,
		class: class_name = "",
	}: {
		children: Snippet<[{ move_hover: (event: Event) => void }]>;
		class?: string;
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

	const move_hover = (event: Event) => {
		if (!(event.currentTarget instanceof HTMLElement) || !surface) return;
		if (event.type === "focus") {
			if (!has_seen_focus) {
				has_seen_focus = true;
				return;
			}
		}

		const surface_rect = surface.getBoundingClientRect();
		const target_rect = event.currentTarget.getBoundingClientRect();

		animated = visible;
		height = target_rect.height;
		left = target_rect.left - surface_rect.left;
		top = target_rect.top - surface_rect.top;
		visible = true;
		width = target_rect.width;
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
	<div class="relative z-1">
		{@render children({ move_hover })}
	</div>
</div>

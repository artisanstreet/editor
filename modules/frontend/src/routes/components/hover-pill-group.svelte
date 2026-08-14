<script lang="ts">
	import type { Snippet } from "svelte";
	import type { PillHover } from "./hover-pill.svelte";

	let {
		children,
		class: class_name = "",
	}: {
		children: Snippet<[{ hover: PillHover }]>;
		class?: string;
	} = $props();

	/**
	 * The distributed sibling of `dropdown-hover-surface`: that component owns
	 * one pill under one surface, which cannot cross an isolated card — a glass
	 * housing is its own stacking context, so a pill outside it can never paint
	 * between the card's material and its rows. This group therefore owns only
	 * the hover state, and every card renders its own `HoverPill` inside its
	 * housing at the same shared geometry, so the copies read as one pill
	 * sliding across the gap between cards.
	 */
	let surface = $state<HTMLElement>();
	let active_target = $state.raw<HTMLElement | undefined>(undefined);
	let animated = $state(false);
	let geometry_version = $state(0);

	const clear_hover = () => {
		active_target = undefined;
		animated = false;
	};

	const apply_hover = (target: HTMLElement) => {
		if (surface === undefined || !surface.contains(target)) {
			clear_hover();
			return;
		}
		animated = active_target !== undefined;
		active_target = target;
		geometry_version += 1;
	};

	const move_hover = (event: Event) => {
		if (event.currentTarget instanceof HTMLElement) apply_hover(event.currentTarget);
	};

	const hover: PillHover = {
		get animated() {
			return animated;
		},
		get target() {
			return active_target;
		},
		get version() {
			return geometry_version;
		},
		move: move_hover,
	};

	const clear_departed_focus = (event: FocusEvent) => {
		if (event.relatedTarget instanceof Node && surface?.contains(event.relatedTarget)) return;
		clear_hover();
	};

	/**
	 * A keyed row can move or leave while the pointer stays inside the group,
	 * which emits no `pointerleave`, and a card's scroller can carry the held
	 * row away under a resting pointer. Reconcile after mutations, resizes, and
	 * captured scrolls so no pill holds stale geometry.
	 */
	const observe_hover_target = (element: HTMLElement) => {
		const reconcile = () => {
			if (active_target === undefined) return;
			if (!element.contains(active_target)) {
				clear_hover();
				return;
			}
			if (!active_target.matches(":hover") && !active_target.matches(":focus-within")) {
				clear_hover();
				return;
			}
			geometry_version += 1;
		};
		const mutation_observer = new MutationObserver(reconcile);
		const resize_observer = new ResizeObserver(reconcile);
		mutation_observer.observe(element, { childList: true, subtree: true });
		resize_observer.observe(element);
		element.addEventListener("scroll", reconcile, { capture: true, passive: true });

		return () => {
			mutation_observer.disconnect();
			resize_observer.disconnect();
			element.removeEventListener("scroll", reconcile, { capture: true });
		};
	};
</script>

<div
	bind:this={surface}
	class={`relative ${class_name}`}
	role="presentation"
	onpointerleave={clear_hover}
	onfocusout={clear_departed_focus}
	{@attach observe_hover_target}
>
	{@render children({ hover })}
</div>

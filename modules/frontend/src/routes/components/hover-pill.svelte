<script lang="ts" module>
	/**
	 * The shared hover contract a {@link file://./hover-pill-group.svelte} hands
	 * its children: the row currently under the pointer, whether the next
	 * placement should tween, and a version that bumps whenever geometry must be
	 * re-measured. Rows report themselves through `move`.
	 */
	export interface PillHover {
		readonly animated: boolean;
		readonly move: (event: Event) => void;
		readonly target: HTMLElement | undefined;
		readonly version: number;
	}
</script>

<script lang="ts">
	let { hover }: { readonly hover: PillHover } = $props();

	let element = $state<HTMLElement>();

	/**
	 * Every pill in the group projects the same hovered row, each in its own
	 * card's coordinates, so the copies overlap into what reads as one pill
	 * travelling between cards while each card's overflow clips its copy at the
	 * glass edge.
	 *
	 * Rect difference rather than offset walks: the target may live in a sibling
	 * card, so no offsetParent chain reaches it, while client rects share every
	 * ancestor transform and scroll offset, which therefore cancel out. The one
	 * requirement is a borderless containing block, since the rect includes the
	 * border the absolute origin sits inside.
	 */
	const geometry = $derived.by(() => {
		void hover.version;
		const target = hover.target;
		const anchor = element?.offsetParent;
		if (target === undefined || anchor == null) return undefined;
		const target_rect = target.getBoundingClientRect();
		const anchor_rect = anchor.getBoundingClientRect();

		return {
			height: target.offsetHeight,
			left: target_rect.left - anchor_rect.left,
			top: target_rect.top - anchor_rect.top,
			width: target.offsetWidth,
		};
	});
</script>

<div
	bind:this={element}
	class="docs-sidebar-hover-highlight"
	data-active={geometry !== undefined}
	data-animate={hover.animated}
	aria-hidden="true"
	style={geometry === undefined
		? ""
		: `--docs-sidebar-hover-x: ${geometry.left}px; --docs-sidebar-hover-y: ${geometry.top}px; --docs-sidebar-hover-width: ${geometry.width}px; --docs-sidebar-hover-height: ${geometry.height}px;`}
></div>

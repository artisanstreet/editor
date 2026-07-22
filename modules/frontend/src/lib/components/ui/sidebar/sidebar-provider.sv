<script lang="ts">
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import { get_sidebar_flip_translation } from "$lib/client/sidebar-motion.js";
	import { cn, type WithElementRef } from "$lib/utils.js";
	import { Effect } from "effect";
	import { onDestroy, tick } from "svelte";
	import type { HTMLAttributes } from "svelte/elements";
	import {
		sidebar_cookie_max_age,
		sidebar_cookie_name,
		sidebar_width,
		sidebar_width_icon,
	} from "$lib/components/ui/sidebar/constants.js";
	import { set_sidebar } from "$lib/components/ui/sidebar/context.svelte.js";

	let {
		ref = $bindable(null),
		open = $bindable(true),
		mobile_breakpoint,
		onOpenChange: on_open_change = () => {},
		class: class_name,
		style,
		children,
		...rest_props
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		open?: boolean;
		mobile_breakpoint?: number;
		onOpenChange?: (open: boolean) => void;
	} = $props();

	type FlipTarget = {
		element: HTMLElement;
		left: number;
		top: number;
	};

	const sidebar_layout_duration_ms = 300;
	const flip_animations = new Map<HTMLElement, Animation>();
	let layout_change_id = 0;
	let overflow_cleanup_timeout: ReturnType<typeof setTimeout> | undefined;

	const get_flip_targets = () => {
		if (!ref) {
			return [];
		}

		return [
			ref.querySelector<HTMLElement>('[data-slot="sidebar-inset"]'),
			ref.querySelector<HTMLElement>('[data-slot="sidebar-trigger"]'),
		]
			.filter((element): element is HTMLElement => element !== null)
			.map((element) => {
				const rect = element.getBoundingClientRect();

				return { element, left: rect.left, top: rect.top };
			});
	};

	const stop_flip_animations = (targets: FlipTarget[]) => {
		for (const { element } of targets) {
			flip_animations.get(element)?.cancel();
			flip_animations.delete(element);
		}
	};

	const play_flip_animations = (before_targets: FlipTarget[]) => {
		if (
			globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches ||
			typeof Element.prototype.animate !== "function"
		) {
			return;
		}

		for (const before of before_targets) {
			if (!before.element.isConnected) {
				continue;
			}

			const after = before.element.getBoundingClientRect();
			const translation = get_sidebar_flip_translation(before, after);

			if (translation.x === 0 && translation.y === 0) {
				continue;
			}

			const animation = before.element.animate(
				[
					{ translate: `${translation.x}px ${translation.y}px` },
					{ translate: "0 0" },
				],
				{
					duration: sidebar_layout_duration_ms,
					easing: "cubic-bezier(0.22, 1, 0.36, 1)",
				}
			);

			flip_animations.set(before.element, animation);
			const cleanup_animation = () => {
				if (flip_animations.get(before.element) === animation) {
					flip_animations.delete(before.element);
				}
			};

			void animation.finished.then(cleanup_animation, cleanup_animation);
		}
	};

	const set_open_with_flip = (value: boolean) => {
		const before_targets = get_flip_targets();
		stop_flip_animations(before_targets);
		const change_id = ++layout_change_id;

		if (ref) {
			ref.dataset.sidebarFlipActive = "true";
		}

		clearTimeout(overflow_cleanup_timeout);
		overflow_cleanup_timeout = setTimeout(() => {
			if (change_id === layout_change_id && ref) {
				delete ref.dataset.sidebarFlipActive;
			}
		}, sidebar_layout_duration_ms);

		open = value;
		on_open_change(value);

		void tick().then(() => {
			if (change_id === layout_change_id) {
				play_flip_animations(before_targets);
			}
		});

		Effect.runSync(
			Effect.sync(() => {
				document.cookie = `${sidebar_cookie_name}=${open}; path=/; max-age=${sidebar_cookie_max_age}`;
			})
		);
	};

	const sidebar = set_sidebar({
		mobile_breakpoint: () => mobile_breakpoint,
		open: () => open,
		set_open: set_open_with_flip,
	});

	onDestroy(() => {
		clearTimeout(overflow_cleanup_timeout);

		for (const animation of flip_animations.values()) {
			animation.cancel();
		}
	});
</script>

<svelte:window onkeydown={sidebar.handle_shortcut_keydown} />

<Tooltip.Provider delayDuration={0}>
	<div
		data-slot="sidebar-wrapper"
		style="--sidebar-width: {sidebar_width}; --sidebar-width-icon: {sidebar_width_icon}; {style}"
		class={cn(
			"group/sidebar-wrapper has-data-[variant=inset]:bg-background flex min-h-svh w-full data-[sidebar-flip-active=true]:overflow-x-clip",
			class_name
		)}
		bind:this={ref}
		{...rest_props}
	>
		{@render children?.()}
	</div>
</Tooltip.Provider>

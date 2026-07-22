<script lang="ts">
	import { Button } from "$lib/components/ui/button/index.js";
	import LayoutSidebar from "@tabler/icons-svelte/icons/layout-sidebar";
	import { cn } from "$lib/utils.js";
	import { play } from "cuelume";
	import { Effect, type Fiber } from "effect";
	import { onDestroy } from "svelte";
	import type { ComponentProps } from "svelte";
	import { use_sidebar } from "$lib/components/ui/sidebar/context.svelte.js";

	let {
		ref = $bindable(null),
		class: class_name,
		children,
		onclick,
		style,
		...rest_props
	}: ComponentProps<typeof Button> & {
		onclick?: (e: MouseEvent) => void;
	} = $props();

	const sidebar = use_sidebar();
	let is_stretching = $state(false);
	let stretch_direction = $state<"open" | "close">("open");
	let stretch_x = $state(1.12);
	let stretch_y = $state(0.94);
	let stretch_distance = $state(3);
	let last_toggle_at = 0;
	let stretch_fiber: Fiber.Fiber<void> | undefined;
	let collapse_fiber: Fiber.Fiber<void> | undefined;

	const wait_next_frame = Effect.callback<void>((resume) => {
		const frame = requestAnimationFrame(() => resume(Effect.void));

		return Effect.sync(() => {
			cancelAnimationFrame(frame);
		});
	});

	const get_transition_ms = (value: string) => {
		const trimmed_value = value.trim();

		if (trimmed_value.endsWith("ms")) {
			return Number.parseFloat(trimmed_value);
		}

		if (trimmed_value.endsWith("s")) {
			return Number.parseFloat(trimmed_value) * 1000;
		}

		return 0;
	};

	const get_transition_values_ms = (value: string) =>
		value.split(",").map(get_transition_ms);

	const get_transition_total_ms = (element: HTMLElement) => {
		const styles = getComputedStyle(element);
		const delays = get_transition_values_ms(styles.transitionDelay);
		const durations = get_transition_values_ms(styles.transitionDuration);
		const transition_count = Math.max(delays.length, durations.length);

		return Math.max(
			...Array.from({ length: transition_count }, (_, index) => {
				const delay = delays[index] ?? delays.at(-1) ?? 0;
				const duration = durations[index] ?? durations.at(-1) ?? 0;

				return delay + duration;
			}),
			0,
		);
	};

	const get_sidebar_children_exit_ms = () => {
		const sidebar_root = ref?.closest<HTMLElement>('[data-slot="sidebar"]');
		const children = sidebar_root
			? [...sidebar_root.querySelectorAll<HTMLElement>(".t-sidebar-child")]
			: [];

		return Math.ceil(Math.max(...children.map(get_transition_total_ms), 0));
	};

	const play_stretch_animation = Effect.gen(function* () {
		yield* Effect.callback<void>((resume) => {
			const frame = requestAnimationFrame(() => resume(Effect.void));

			return Effect.sync(() => {
				cancelAnimationFrame(frame);
			});
		});

		is_stretching = true;

		yield* Effect.sleep("420 millis");

		is_stretching = false;
	});

	function replay_velocity_stretch() {
		const will_open =
			sidebar.motion_phase === "children-exiting"
				? true
				: sidebar.is_mobile
					? !sidebar.open_mobile
					: !sidebar.open;
		const now = performance.now();
		const elapsed = last_toggle_at === 0 ? 420 : Math.min(now - last_toggle_at, 420);
		const velocity = 1 - elapsed / 420;

		last_toggle_at = now;
		stretch_direction = will_open ? "open" : "close";
		stretch_x = 1.1 + velocity * 0.08;
		stretch_y = 0.95 - velocity * 0.04;
		stretch_distance = 2.5 + velocity * 2.5;
		is_stretching = false;

		stretch_fiber?.interruptUnsafe();
		stretch_fiber = Effect.runFork(play_stretch_animation);
	}

	const queue_sidebar_collapse = Effect.gen(function* () {
		sidebar.set_motion_phase("children-exiting");

		yield* wait_next_frame;
		yield* Effect.sleep(`${get_sidebar_children_exit_ms()} millis`);

		sidebar.set_open(false);
		sidebar.set_motion_phase("idle");
	});

	function toggle_sidebar() {
		collapse_fiber?.interruptUnsafe();

		if (sidebar.is_mobile) {
			sidebar.toggle();
			return;
		}

		play("toggle");

		if (sidebar.motion_phase === "children-exiting") {
			sidebar.set_motion_phase("idle");
			return;
		}

		if (sidebar.open) {
			collapse_fiber = Effect.runFork(queue_sidebar_collapse);
			return;
		}

		sidebar.set_motion_phase("idle");
		sidebar.set_open(true);
	}

	onDestroy(() => {
		stretch_fiber?.interruptUnsafe();
		collapse_fiber?.interruptUnsafe();
	});
</script>

<Button
	bind:ref
	data-sidebar="trigger"
	data-slot="sidebar-trigger"
	variant="ghost"
	size="icon-sm"
	class={cn("cn-sidebar-trigger t-velocity-stretch", class_name)}
	type="button"
	data-stretching={is_stretching}
	data-stretch-direction={stretch_direction}
	style={`--sidebar-stretch-x: ${stretch_x.toFixed(3)}; --sidebar-stretch-y: ${stretch_y.toFixed(3)}; --sidebar-stretch-distance: ${stretch_distance.toFixed(2)}px; ${style ?? ""}`}
	onclick={(event) => {
		onclick?.(event);
		replay_velocity_stretch();
		toggle_sidebar();
	}}
	{...rest_props}
>
	{#if children}
		{@render children()}
	{:else}
		<LayoutSidebar />
	{/if}
	<span class="sr-only">Toggle Sidebar</span>
</Button>

<script lang="ts">
	/**
	 * Orbit, on physics.
	 *
	 * The first pass had the idea — recency as distance — and then made it a
	 * picture you click. Two things were missing, and they are the same thing:
	 * the arc did not know where the pointer was.
	 *
	 *   It follows you now. The whole arc leans towards the cursor in three
	 *   dimensions, on a spring soft enough to lag behind the hand rather than
	 *   track it. Tying a transform straight to a mouse position reads as
	 *   mechanical, because nothing with mass changes direction instantly; the
	 *   spring is what makes it read as an object rather than a readout.
	 *
	 *   It can be thrown. Drag it, flick it, or scroll it, and the arc spins
	 *   under the hand and settles on whatever it was heading for. Velocity is
	 *   carried through the release, so a flick counts for more than where the
	 *   fingers happened to stop, and carried through a change of target, so a
	 *   fast run of clicks retargets mid-flight instead of restarting.
	 *
	 * Duration-based easing cannot do either. A curve has to finish before it can
	 * be told to go somewhere else. So position is integrated frame by frame from
	 * a damped spring, the loop parks itself the moment everything has settled,
	 * and no element carries a CSS transition on the property being driven.
	 *
	 * The rest is legibility, which was the other half of the complaint. The arc
	 * is drawn, its ends are labelled, hovering a project previews it without
	 * committing to it, and the whole thing answers the keyboard.
	 */
	import { DraftProjects, ThreadsFor } from "../mock";
	import { SpringSettled, SpringStep, type SpringState } from "../motion";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";

	const total = DraftProjects.length;
	/** Slots run ±half, so the far side of the arc is the wrap seam. */
	const half = total / 2;
	/** Radians per project: seven of them span ~2.4rad, a bit under a third turn. */
	const spread = 0.34;
	const radius = 208;
	/** The arc is far wider than it is tall; a circle would waste the height. */
	const flatten = 1.85;
	/** How far the pointer drags to advance the arc by one project. */
	const drag_pitch = 150;
	/** Wheel notches per project. */
	const wheel_pitch = 40;

	/**
	 * Read once. Everything below branches on it — no loop, no lean, no flick —
	 * and re-reading it per frame would be sixty media-query evaluations a second
	 * to answer a question that changes about as often as a preference pane does.
	 */
	const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

	let offset = $state(0);
	let target = $state(0);
	let lean_x = $state(0);
	let lean_y = $state(0);
	let hovered = $state<number | undefined>(undefined);
	let dragging = $state(false);
	let region: HTMLElement | undefined = undefined;

	/**
	 * Springs live outside `$state`: their velocity is read sixty times a second
	 * and never rendered, and the values above are the one thing per frame the
	 * template actually needs.
	 */
	let spin: SpringState = { value: 0, velocity: 0 };
	let tilt_x: SpringState = { value: 0, velocity: 0 };
	let tilt_y: SpringState = { value: 0, velocity: 0 };
	let aim_x = 0;
	let aim_y = 0;
	let frame = 0;
	let last_time = 0;
	let box: DOMRect | undefined = undefined;
	let drag_from = 0;
	let drag_offset = 0;
	let drag_travel = 0;
	let drag_velocity = 0;
	let drag_time = 0;
	let wheel_debt = 0;

	const Frame = (time: number) => {
		const dt = last_time === 0 ? 1 / 60 : Math.min(0.032, (time - last_time) / 1000);
		last_time = time;

		/** While the hand is on it, the hand is the integrator. */
		if (!dragging) {
			spin = SpringStep(spin, target, dt, 150, 22);
			offset = spin.value;
		}
		/** Slack and underdamped by comparison: the lean should trail the cursor. */
		tilt_x = SpringStep(tilt_x, aim_x, dt, 70, 14);
		tilt_y = SpringStep(tilt_y, aim_y, dt, 70, 14);
		lean_x = tilt_x.value;
		lean_y = tilt_y.value;

		const resting =
			!dragging &&
			SpringSettled(spin, target) &&
			SpringSettled(tilt_x, aim_x) &&
			SpringSettled(tilt_y, aim_y);
		if (resting) {
			spin = { value: target, velocity: 0 };
			offset = target;
			frame = 0;
			last_time = 0;
			return;
		}
		frame = requestAnimationFrame(Frame);
	};

	const Wake = () => {
		if (reduced) {
			/** With no loop to run, the target is the position — unless a hand is on it. */
			if (!dragging) offset = target;
			return;
		}
		if (frame !== 0) return;
		last_time = 0;
		frame = requestAnimationFrame(Frame);
	};

	$effect(() => () => {
		if (frame !== 0) cancelAnimationFrame(frame);
	});

	/** The shortest way round: a project three to the left is not four to the right. */
	const Wrap = (value: number) => ((((value + half) % total) + total) % total) - half;

	const Place = (slot: number) => {
		const angle = slot * spread;
		return { x: Math.sin(angle) * radius * flatten, y: -Math.cos(angle) * radius };
	};

	/**
	 * Drawn from the same function that places the tiles, so the guide cannot
	 * describe an arc the projects are not actually sitting on.
	 */
	const guide = Array.from({ length: 49 }, (_, step) => {
		const spot = Place(-half + (step / 48) * total);
		return `${step === 0 ? "M" : "L"}${spot.x.toFixed(1)} ${spot.y.toFixed(1)}`;
	}).join(" ");

	const Tile = (index: number) => {
		const slot = Wrap(index - offset);
		const distance = Math.abs(slot) / half;
		const spot = Place(slot);
		return {
			distance,
			/**
			 * Quadratic, so it reaches zero exactly at the seam. A project crossing
			 * from one end of the arc to the other has to be invisible while it does
			 * it, or the wrap is a teleport.
			 */
			opacity: Math.max(0, 1 - distance * distance),
			scale: 1 - distance * 0.34,
			x: spot.x,
			y: spot.y,
			z: Math.round(100 - distance * 90),
		};
	};

	const active_index = $derived(((Math.round(target) % total) + total) % total);
	const active = $derived(DraftProjects[active_index] ?? DraftProjects[0]);
	const previewed = $derived(DraftProjects[hovered ?? active_index] ?? DraftProjects[0]);

	const arc_transform = $derived(
		reduced
			? "none"
			: `translate3d(${(lean_x * 12).toFixed(2)}px, ${(lean_y * 7).toFixed(2)}px, 0) rotateX(${(lean_y * -5).toFixed(2)}deg) rotateY(${(lean_x * 6).toFixed(2)}deg)`,
	);

	const Center = (index: number) => {
		/** A drag that ended on a tile is still a drag, not a click on it. */
		if (drag_travel > 4) return;
		const current = Math.round(target);
		target = current + Wrap(index - (((current % total) + total) % total));
		Wake();
	};

	const OnPointerDown = (event: PointerEvent) => {
		if (dragging || !event.isPrimary || region === undefined) return;
		dragging = true;
		hovered = undefined;
		drag_from = event.clientX;
		drag_offset = spin.value;
		drag_travel = 0;
		drag_velocity = 0;
		drag_time = event.timeStamp;
		region.setPointerCapture(event.pointerId);
	};

	const OnPointerMove = (event: PointerEvent) => {
		if (region === undefined) return;
		/** Measured on entry, not per event: a rect read is a layout read. */
		if (box === undefined) box = region.getBoundingClientRect();

		aim_x = Math.max(-1, Math.min(1, ((event.clientX - box.left) / box.width) * 2 - 1));
		aim_y = Math.max(-1, Math.min(1, ((event.clientY - box.top) / box.height) * 2 - 1));

		if (dragging) {
			const travelled = event.clientX - drag_from;
			drag_travel = Math.max(drag_travel, Math.abs(travelled));
			const next = drag_offset - travelled / drag_pitch;
			const elapsed = Math.max(1, event.timeStamp - drag_time);
			drag_velocity = ((next - spin.value) / elapsed) * 1000;
			drag_time = event.timeStamp;
			spin = { value: next, velocity: drag_velocity };
			offset = next;
		}
		Wake();
	};

	const OnPointerUp = (event: PointerEvent) => {
		if (!dragging || region === undefined) return;
		dragging = false;
		region.releasePointerCapture(event.pointerId);
		/**
		 * Where it was going, not where it stopped — and no further than two
		 * projects on, so a violent flick still lands somewhere you meant.
		 */
		const projected = spin.value + drag_velocity * 0.12;
		target = Math.round(
			Math.max(spin.value - 2, Math.min(spin.value + 2, reduced ? spin.value : projected)),
		);
		Wake();
	};

	const OnWheel = (event: WheelEvent) => {
		const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
		wheel_debt += delta;
		const steps = Math.trunc(wheel_debt / wheel_pitch);
		if (steps === 0) return;
		wheel_debt -= steps * wheel_pitch;
		target += steps;
		Wake();
	};

	/** The cached rect is only wrong when the page has changed shape under it. */
	const OnResize = () => {
		box = undefined;
	};

	/** `←` `→` belong to the gallery and `h` hides it, so stepping is `j` `k`. */
	const OnKeydown = (event: KeyboardEvent) => {
		if (event.metaKey || event.ctrlKey || event.altKey) return;
		if (event.key === "ArrowDown" || event.key === "j") target += 1;
		else if (event.key === "ArrowUp" || event.key === "k") target -= 1;
		else return;
		Wake();
		event.preventDefault();
	};
</script>

<!--
	The lean is tracked at the window rather than on the arc: the arc should
	notice the cursor from across the page, which is most of what makes it read
	as an object in the room rather than a control waiting to be touched.
-->
<svelte:window onkeydown={OnKeydown} onpointermove={OnPointerMove} onresize={OnResize} />

<div class="dz-vignette relative grid h-full place-items-center overflow-hidden p-8">
	<div class="w-full max-w-[40rem]">
		<div
			aria-label="Projects by recency"
			bind:this={region}
			class="dz-enter relative h-[17.5rem] w-full touch-none select-none"
			class:cursor-grab={!dragging}
			class:cursor-grabbing={dragging}
			onpointercancel={OnPointerUp}
			onpointerdown={OnPointerDown}
			onpointerup={OnPointerUp}
			onwheel={OnWheel}
			role="group"
			style:perspective="1100px"
			style:perspective-origin="50% 100%"
		>
			<!-- Zero-sized: it is an origin, and everything on the arc is placed from it. -->
			<div
				class="absolute bottom-0 left-1/2 h-0 w-0"
				style:transform={arc_transform}
				style:transform-style="preserve-3d"
			>
				<svg
					aria-hidden="true"
					class="pointer-events-none absolute top-0 left-0 overflow-visible"
					height="1"
					width="1"
				>
					<defs>
						<linearGradient id="dz-orbit-guide" x1="0" x2="1" y1="0" y2="0">
							<stop offset="0%" stop-color="var(--surface-500)" stop-opacity="0" />
							<stop offset="50%" stop-color="var(--surface-500)" stop-opacity="0.55" />
							<stop offset="100%" stop-color="var(--surface-500)" stop-opacity="0" />
						</linearGradient>
					</defs>
					<path
						d={guide}
						fill="none"
						stroke="url(#dz-orbit-guide)"
						stroke-dasharray="1.5 6"
						stroke-linecap="round"
						stroke-width="1"
					/>
				</svg>

				<!-- The metaphor, said once in words, so nobody has to infer it. -->
				<span
					class="pointer-events-none absolute top-0 left-0 font-mono text-[10px] tracking-wide text-surface-600"
					style:transform={`translate(-50%, ${(Place(0).y - 34).toFixed(0)}px)`}
				>
					now
				</span>
				{#each [-1, 1] as side (side)}
					<span
						class="pointer-events-none absolute top-0 left-0 font-mono text-[10px] tracking-wide text-surface-700"
						style:transform={`translate(calc(-50% + ${(Place(half * side).x * 1.1).toFixed(0)}px), ${(Place(half * side).y + 14).toFixed(0)}px)`}
					>
						older
					</span>
				{/each}

				{#each DraftProjects as project, index (project.project_id)}
					{@const tile = Tile(index)}
					<button
						class="dz-focus dz-lift-host absolute top-0 left-0 h-[5.5rem] origin-top rounded-2xl outline-none"
						onclick={() => Center(index)}
						onpointerenter={() => (hovered = dragging ? undefined : index)}
						onpointerleave={() => (hovered = undefined)}
						style:opacity={tile.opacity}
						style:pointer-events={tile.opacity < 0.1 ? "none" : "auto"}
						style:transform={`translate(calc(-50% + ${tile.x.toFixed(1)}px), ${tile.y.toFixed(1)}px) scale(${tile.scale.toFixed(3)})`}
						style:z-index={tile.z}
						type="button"
					>
						<span class="dz-lift flex flex-col items-center gap-2">
							<!-- Colour is identity, and identity fades with distance like everything else. -->
							<span class="dz-press" style:filter={`grayscale(${(tile.distance * 55).toFixed(0)}%)`}>
								<Monogram class="size-12 rounded-2xl text-xs" {project} />
							</span>
							<span class="max-w-28 truncate text-[11px] whitespace-nowrap text-muted-foreground">
								{project.name}
							</span>
							<span
								class="text-[10px] text-surface-500 transition-opacity duration-150"
								style:opacity={tile.distance < 0.1 ? 1 : 0}
							>
								{project.last_used}
							</span>
						</span>
					</button>
				{/each}
			</div>
		</div>

		<!--
			Bound to whatever the pointer is over, so a project can be inspected
			without being chosen. The composer below stays with the selection: a
			field whose subject changes as the cursor drifts is a field you cannot
			trust.
		-->
		{#key previewed.project_id}
			<p class="dz-quiet mb-3 text-center text-sm text-muted-foreground">
				{ThreadsFor(previewed.project_id).length} open in
				<span class="text-foreground-extra">{previewed.name}</span>
				<span class="text-surface-600">· {previewed.branch} · {previewed.last_used}</span>
			</p>
		{/key}

		<div class="dz-enter" style:--dz-delay="240ms">
			<Composer placeholder={`What are we doing in ${active.name}?`} tall />
		</div>

		<p class="mt-3 text-center font-mono text-[10px] text-surface-700">
			drag or scroll to spin · j k to step
		</p>
	</div>
</div>

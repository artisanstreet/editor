<script lang="ts">
	import { onDestroy } from "svelte";
	import { Button } from "$lib/components/ui/button";
	import { NativeSelect, NativeSelectOption } from "$lib/components/ui/native-select";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { conversation_diagnostics_enabled } from "$lib/conversation/diagnostics";
	import {
		changed_files_style_config,
		reset_changed_files_style_config,
		reset_user_message_style_config,
		update_changed_files_style_config,
		update_user_message_style_config,
		user_message_style_config,
		type ChangedFilesStyleConfig,
		type UserMessageStyleConfig,
	} from "$lib/conversation-style-config";
	import {
		reset_shader_config,
		shader_config,
		surface_tokens,
		update_shader_config,
		type ShaderConfig,
		type SurfaceToken,
	} from "$lib/shader-config";

	type NumberKey = {
		[K in keyof ShaderConfig]: ShaderConfig[K] extends number ? K : never;
	}[keyof ShaderConfig];
	type ColorKey = {
		[K in keyof ShaderConfig]: ShaderConfig[K] extends SurfaceToken ? K : never;
	}[keyof ShaderConfig];
	let debug_overlay: HTMLCanvasElement | undefined;
	let debug_overlay_frame: number | undefined;
	let show_padding_guides = $state(false);
	let show_text_box_trim = $state(false);
	let show_wireframes = $state(false);

	const remove_debug_overlay = () => {
		if (debug_overlay_frame !== undefined) {
			cancelAnimationFrame(debug_overlay_frame);
			debug_overlay_frame = undefined;
		}
		debug_overlay?.remove();
		debug_overlay = undefined;
	};

	const draw_debug_overlay = () => {
		if (!show_wireframes && !show_padding_guides) {
			remove_debug_overlay();
			return;
		}

		debug_overlay ??= Object.assign(document.createElement("canvas"), {
			ariaHidden: "true",
			className: "debug-layout-overlay",
		});
		if (!debug_overlay.isConnected) {
			document.body.append(debug_overlay);
		}

		const pixel_ratio = window.devicePixelRatio;
		const width = window.innerWidth;
		const height = window.innerHeight;
		debug_overlay.width = Math.round(width * pixel_ratio);
		debug_overlay.height = Math.round(height * pixel_ratio);
		debug_overlay.style.width = `${width}px`;
		debug_overlay.style.height = `${height}px`;

		const context = debug_overlay.getContext("2d");
		if (context === null) {
			remove_debug_overlay();
			return;
		}

		context.setTransform(pixel_ratio, 0, 0, pixel_ratio, 0, 0);
		context.clearRect(0, 0, width, height);
		context.lineWidth = 1;

		for (const element of document.body.querySelectorAll<HTMLElement>("*")) {
			if (
				element === debug_overlay ||
				element.closest("svg") !== null ||
				element.tagName === "SCRIPT" ||
				element.tagName === "STYLE"
			) {
				continue;
			}

			const bounds = element.getBoundingClientRect();
			if (
				bounds.width < 1 ||
				bounds.height < 1 ||
				bounds.right < 0 ||
				bounds.bottom < 0 ||
				bounds.left > width ||
				bounds.top > height
			) {
				continue;
			}

			const left = Math.round(bounds.left) + 0.5;
			const top = Math.round(bounds.top) + 0.5;
			const right = Math.round(bounds.right) - 0.5;
			const bottom = Math.round(bounds.bottom) - 0.5;

			if (show_wireframes) {
				context.strokeStyle = "oklch(0.75 0.14 230 / 0.48)";
				context.strokeRect(left, top, right - left, bottom - top);
			}

			if (show_padding_guides) {
				const has_element_child = Array.from(element.children).some(
					(child) =>
						child instanceof HTMLElement &&
						child !== debug_overlay &&
						child.closest("svg") === null,
				);
				if (!has_element_child) {
					continue;
				}

				const style = getComputedStyle(element);
				const inset_left = Number.parseFloat(style.borderLeftWidth) + Number.parseFloat(style.paddingLeft);
				const inset_top = Number.parseFloat(style.borderTopWidth) + Number.parseFloat(style.paddingTop);
				const inset_right =
					Number.parseFloat(style.borderRightWidth) + Number.parseFloat(style.paddingRight);
				const inset_bottom =
					Number.parseFloat(style.borderBottomWidth) + Number.parseFloat(style.paddingBottom);
				if (Math.max(inset_left, inset_top, inset_right, inset_bottom) < 1) {
					continue;
				}

				context.strokeStyle = "oklch(0.78 0.16 55 / 0.82)";
				context.beginPath();
				for (const [parent_x, parent_y, child_x, child_y] of [
					[left, top, left + inset_left, top + inset_top],
					[right, top, right - inset_right, top + inset_top],
					[right, bottom, right - inset_right, bottom - inset_bottom],
					[left, bottom, left + inset_left, bottom - inset_bottom],
				]) {
					if (Math.hypot(child_x - parent_x, child_y - parent_y) < 1) {
						continue;
					}
					context.moveTo(parent_x, parent_y);
					context.lineTo(child_x, child_y);
				}
				context.stroke();
			}
		}

		debug_overlay_frame = requestAnimationFrame(draw_debug_overlay);
	};

	const set_number = (key: NumberKey, value: string) =>
		update_shader_config({ [key]: Number(value) });
	const set_color = (key: ColorKey, value: string) =>
		update_shader_config({ [key]: value as SurfaceToken });
	const set_user_message_color = (
		key: Exclude<keyof UserMessageStyleConfig, "use_card">,
		value: string,
	) => update_user_message_style_config({ [key]: value as SurfaceToken });
	const set_changed_files_color = (
		key: Exclude<keyof ChangedFilesStyleConfig, "use_card">,
		value: string,
	) => update_changed_files_style_config({ [key]: value as SurfaceToken });
	const reset_controls = () => {
		reset_shader_config();
		reset_user_message_style_config();
		reset_changed_files_style_config();
		show_padding_guides = false;
		show_text_box_trim = false;
		show_wireframes = false;
		conversation_diagnostics_enabled.set(false);
	};

	$effect(() => {
		void show_wireframes;
		void show_padding_guides;

		if (debug_overlay_frame === undefined) {
			debug_overlay_frame = requestAnimationFrame(draw_debug_overlay);
		}
	});

	$effect(() => {
		document.documentElement.classList.toggle("debug-text-box-trim", show_text_box_trim);
	});

	onDestroy(() => {
		remove_debug_overlay();
		document.documentElement.classList.remove("debug-text-box-trim");
	});
</script>

{#snippet color_control(label: string, key: ColorKey, value: SurfaceToken)}
	<label class="grid grid-cols-[5.5rem_minmax(0,1fr)] items-center gap-2 text-xs">
		<span class="text-muted-foreground">{label}</span>
		<NativeSelect
			size="sm"
			class="w-full"
			value={value}
			onchange={(event) => set_color(key, event.currentTarget.value)}
			aria-label={label}
		>
			{#each surface_tokens as token}
				<NativeSelectOption value={token}>{token}</NativeSelectOption>
			{/each}
		</NativeSelect>
	</label>
{/snippet}

{#snippet changed_files_color_control(label: string, key: Exclude<keyof ChangedFilesStyleConfig, "use_card">, value: SurfaceToken)}
	<label class="grid grid-cols-[5.5rem_minmax(0,1fr)] items-center gap-2 text-xs">
		<span class="text-muted-foreground">{label}</span>
		<NativeSelect
			size="sm"
			class="w-full"
			{value}
			onchange={(event) => set_changed_files_color(key, event.currentTarget.value)}
			aria-label={label}
		>
			{#each surface_tokens as token}
				<NativeSelectOption value={token}>{token}</NativeSelectOption>
			{/each}
		</NativeSelect>
	</label>
{/snippet}

{#snippet user_message_color_control(label: string, key: Exclude<keyof UserMessageStyleConfig, "use_card">, value: SurfaceToken)}
	<label class="grid grid-cols-[5.5rem_minmax(0,1fr)] items-center gap-2 text-xs">
		<span class="text-muted-foreground">{label}</span>
		<NativeSelect
			size="sm"
			class="w-full"
			{value}
			onchange={(event) => set_user_message_color(key, event.currentTarget.value)}
			aria-label={label}
		>
			{#each surface_tokens as token}
				<NativeSelectOption value={token}>{token}</NativeSelectOption>
			{/each}
		</NativeSelect>
	</label>
{/snippet}

{#snippet number_control(label: string, key: NumberKey, value: number, min: number, max: number, step: number)}
	<label class="grid gap-1.5 text-xs">
		<span class="flex items-center justify-between gap-2">
			<span class="text-muted-foreground">{label}</span>
			<output class="tabular-nums text-foreground">{value.toFixed(step < 0.1 ? 2 : 1)}</output>
		</span>
		<input
			type="range"
			{min}
			{max}
			{step}
			value={value}
			oninput={(event) => set_number(key, event.currentTarget.value)}
			class="shader-range w-full accent-foreground"
		/>
	</label>
{/snippet}

<section class="flex min-h-0 flex-1 flex-col gap-2" aria-label="Shader development controls">
	<div class="flex items-center justify-between gap-2 px-1">
		<div>
			<h2 class="text-sm font-medium">God Rays</h2>
			<p class="text-xs text-muted-foreground">Development controls</p>
		</div>
		<Button variant="ghost" size="xs" onclick={reset_controls}>Reset</Button>
	</div>

	<ScrollArea class="min-h-0 flex-1 rounded-2xl bg-surface-950 card" scrollbarYClasses="w-1">
		<div class="grid gap-6 p-3 pr-4">
			<section class="grid gap-2.5">
				<h3 class="text-xs font-medium text-foreground">Colors</h3>
				{@render color_control("Color 1", "color_1", $shader_config.color_1)}
				{@render color_control("Color 2", "color_2", $shader_config.color_2)}
				{@render color_control("Color 3", "color_3", $shader_config.color_3)}
				{@render color_control("Color 4", "color_4", $shader_config.color_4)}
				{@render color_control("Color 5", "color_5", $shader_config.color_5)}
				{@render color_control("Backing", "color_back", $shader_config.color_back)}
				{@render color_control("Bloom", "color_bloom", $shader_config.color_bloom)}
			</section>

			<section class="grid gap-2.5">
				<h3 class="text-xs font-medium text-foreground">User messages</h3>
				{@render user_message_color_control("From", "from", $user_message_style_config.from)}
				{@render user_message_color_control("To", "to", $user_message_style_config.to)}
				<label class="flex items-center justify-between gap-2 text-xs">
					<span class="text-muted-foreground">Card utility</span>
					<input
						type="checkbox"
						checked={$user_message_style_config.use_card}
						onchange={(event) =>
							update_user_message_style_config({
								use_card: event.currentTarget.checked,
							})}
						class="size-4 accent-foreground"
					/>
				</label>
			</section>

			<section class="grid gap-2.5">
				<h3 class="text-xs font-medium text-foreground">Changed files</h3>
				{@render changed_files_color_control("From", "from", $changed_files_style_config.from)}
				{@render changed_files_color_control("To", "to", $changed_files_style_config.to)}
				<label class="flex items-center justify-between gap-2 text-xs">
					<span class="text-muted-foreground">Card utility</span>
					<input
						type="checkbox"
						checked={$changed_files_style_config.use_card}
						onchange={(event) =>
							update_changed_files_style_config({
								use_card: event.currentTarget.checked,
							})}
						class="size-4 accent-foreground"
					/>
				</label>
			</section>

			<section class="grid gap-2.5">
				<h3 class="text-xs font-medium text-foreground">Debug</h3>
				<label class="flex items-center justify-between gap-2 text-xs">
					<span class="text-muted-foreground">Diagnostics</span>
					<input
						type="checkbox"
						checked={$conversation_diagnostics_enabled}
						onchange={(event) =>
							conversation_diagnostics_enabled.set(event.currentTarget.checked)}
						class="size-4 accent-foreground"
					/>
				</label>
				<label class="flex items-center justify-between gap-2 text-xs">
					<span class="text-muted-foreground">Wireframes</span>
					<input
						type="checkbox"
						bind:checked={show_wireframes}
						class="size-4 accent-foreground"
					/>
				</label>
				<label class="flex items-center justify-between gap-2 text-xs">
					<span class="text-muted-foreground">Padding guides</span>
					<input
						type="checkbox"
						bind:checked={show_padding_guides}
						class="size-4 accent-foreground"
					/>
				</label>
				<label class="flex items-center justify-between gap-2 text-xs">
					<span class="text-muted-foreground">Geometric text trim</span>
					<input
						type="checkbox"
						bind:checked={show_text_box_trim}
						class="size-4 accent-foreground"
					/>
				</label>
			</section>

			<section class="grid gap-3">
				<h3 class="text-xs font-medium text-foreground">Light</h3>
				{@render number_control("Backing opacity", "back_opacity", $shader_config.back_opacity, 0, 1, 0.01)}
				{@render number_control("Bloom", "bloom", $shader_config.bloom, 0, 1, 0.01)}
				{@render number_control("Intensity", "intensity", $shader_config.intensity, 0, 1, 0.01)}
				{@render number_control("Density", "density", $shader_config.density, 0, 1, 0.01)}
				{@render number_control("Spotty", "spotty", $shader_config.spotty, 0, 1, 0.01)}
				{@render number_control("Mid size", "mid_size", $shader_config.mid_size, 0, 1, 0.01)}
				{@render number_control("Mid intensity", "mid_intensity", $shader_config.mid_intensity, 0, 1, 0.01)}
			</section>

			<section class="grid gap-3 pb-1">
				<h3 class="text-xs font-medium text-foreground">Motion and position</h3>
				{@render number_control("Speed", "speed", $shader_config.speed, -2, 2, 0.01)}
				{@render number_control("Scale", "scale", $shader_config.scale, 0.01, 4, 0.01)}
				{@render number_control("Rotation", "rotation", $shader_config.rotation, 0, 360, 1)}
				{@render number_control("Offset X", "offset_x", $shader_config.offset_x, -1, 1, 0.01)}
				{@render number_control("Offset Y", "offset_y", $shader_config.offset_y, -1, 1, 0.01)}
			</section>
		</div>
	</ScrollArea>
</section>

<style>
	.shader-range {
		height: 1rem;
		cursor: pointer;
	}

	:global(.debug-layout-overlay) {
		position: fixed;
		inset: 0;
		z-index: 2147483647;
		pointer-events: none;
	}

	@supports (text-box: trim-both cap alphabetic) {
		:global(
			html.debug-text-box-trim
				body
				:where(
					a,
					button,
					code,
					dd,
					dt,
					h1,
					h2,
					h3,
					h4,
					h5,
					h6,
					label,
					li,
					output,
					p,
					pre,
					small,
					span
				)
		) {
			text-box: trim-both cap alphabetic;
		}
	}
</style>

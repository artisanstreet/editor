import { writable } from "svelte/store";

export type SurfaceToken = `surface-${number}`;

export type ShaderConfig = {
	color_1: SurfaceToken;
	color_2: SurfaceToken;
	color_3: SurfaceToken;
	color_4: SurfaceToken;
	color_5: SurfaceToken;
	color_back: SurfaceToken;
	color_bloom: SurfaceToken;
	back_opacity: number;
	bloom: number;
	intensity: number;
	density: number;
	spotty: number;
	mid_size: number;
	mid_intensity: number;
	speed: number;
	scale: number;
	rotation: number;
	offset_x: number;
	offset_y: number;
};

export const surface_tokens = Array.from(
	{ length: 41 },
	(_, index) => `surface-${index * 25}` as SurfaceToken,
);

export const default_shader_config: ShaderConfig = {
	color_1: "surface-450",
	color_2: "surface-475",
	color_3: "surface-825",
	color_4: "surface-825",
	color_5: "surface-750",
	color_back: "surface-750",
	color_bloom: "surface-25",
	back_opacity: 0,
	bloom: 0,
	intensity: 0,
	density: 0.08,
	spotty: 0.66,
	mid_size: 0.46,
	mid_intensity: 0.11,
	speed: -2,
	scale: 4,
	rotation: 0,
	offset_x: -1,
	offset_y: -0.7,
};

export const shader_config = writable<ShaderConfig>({ ...default_shader_config });

export const update_shader_config = (patch: Partial<ShaderConfig>) =>
	shader_config.update((config) => ({ ...config, ...patch }));

export const reset_shader_config = () => shader_config.set({ ...default_shader_config });

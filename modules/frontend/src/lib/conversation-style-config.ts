import { writable } from "svelte/store";

import type { SurfaceToken } from "$lib/shader-config";

export type GradientCardStyleConfig = {
	from: SurfaceToken;
	to: SurfaceToken;
	use_card: boolean;
};

export type UserMessageStyleConfig = GradientCardStyleConfig;
export type ChangedFilesStyleConfig = GradientCardStyleConfig;

export const default_user_message_style_config: UserMessageStyleConfig = {
	from: "surface-850",
	to: "surface-775",
	use_card: true,
};

export const default_changed_files_style_config: ChangedFilesStyleConfig = {
	from: "surface-925",
	to: "surface-900",
	use_card: true,
};

export const user_message_style_config = writable<UserMessageStyleConfig>({
	...default_user_message_style_config,
});

export const changed_files_style_config = writable<ChangedFilesStyleConfig>({
	...default_changed_files_style_config,
});

export const update_user_message_style_config = (patch: Partial<UserMessageStyleConfig>) =>
	user_message_style_config.update((config) => ({
		...config,
		...patch,
	}));

export const reset_user_message_style_config = () =>
	user_message_style_config.set({ ...default_user_message_style_config });

export const update_changed_files_style_config = (patch: Partial<ChangedFilesStyleConfig>) =>
	changed_files_style_config.update((config) => ({
		...config,
		...patch,
	}));

export const reset_changed_files_style_config = () =>
	changed_files_style_config.set({ ...default_changed_files_style_config });

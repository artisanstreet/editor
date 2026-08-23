import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

import {
	default_desktop_app_icon,
	desktop_app_icon_control_path,
	DesktopAppIconPreference,
	DesktopAppIconSelection,
	type DesktopAppIconPreference as DesktopAppIconPreferenceType,
} from "@artisan/protocol";
import { Effect, Option, Result, Schema } from "effect";

import { app_host } from "./renderer-host";

export interface DesktopAppIconAssets {
	readonly image: string;
	readonly windows_shell: string;
}

export const desktop_app_icon_assets: Readonly<
	Record<DesktopAppIconPreferenceType, DesktopAppIconAssets>
> = {
	"foreground-gradient-symbol": {
		image: "foreground-gradient-symbol.png",
		windows_shell: "foreground-gradient-symbol.ico",
	},
	"plastic-jaw-shading": {
		image: "plastic-jaw-shading.png",
		windows_shell: "plastic-jaw-shading.ico",
	},
};

const DesktopAppIconSettings = Schema.Struct({
	icon: DesktopAppIconPreference,
	version: Schema.Literal(1),
});

export interface DesktopAppIconController {
	readonly Current: () => DesktopAppIconPreferenceType;
	readonly Select: (icon: DesktopAppIconPreferenceType) => Effect.Effect<void, unknown>;
}

/** Missing and stale records resolve to the product default instead of blocking launch. */
export const LoadDesktopAppIconPreference = (path: string) =>
	Effect.gen(function* () {
		const source = yield* Effect.tryPromise(() => readFile(path, "utf8")).pipe(Effect.option);
		if (Option.isNone(source)) return default_desktop_app_icon;
		const parsed = yield* Effect.try(() => JSON.parse(source.value) as unknown).pipe(
			Effect.option,
		);
		if (Option.isNone(parsed)) return default_desktop_app_icon;
		const settings = yield* Schema.decodeUnknownEffect(DesktopAppIconSettings)(
			parsed.value,
		).pipe(Effect.option);
		return Option.isSome(settings) ? settings.value.icon : default_desktop_app_icon;
	});

export const SaveDesktopAppIconPreference = (path: string, icon: DesktopAppIconPreferenceType) =>
	Effect.tryPromise(async () => {
		await mkdir(dirname(path), { recursive: true });
		await writeFile(path, JSON.stringify({ icon, version: 1 }), "utf8");
	});

/**
 * Windows Explorer cannot load a taskbar icon from inside Electron's ASAR.
 * Materialize the packaged ICO under userData, and include its content digest
 * in the filename so an upgraded design cannot be hidden by the shell cache.
 */
export const MaterializeWindowsAppIcon = (
	packaged_root: string,
	writable_root: string,
	icon: DesktopAppIconPreferenceType,
) =>
	Effect.tryPromise(async () => {
		const asset = desktop_app_icon_assets[icon].windows_shell;
		const bytes = await readFile(join(packaged_root, asset));
		const digest = createHash("sha256").update(bytes).digest("hex").slice(0, 12);
		const path = join(writable_root, `${icon}-${digest}.ico`);
		await mkdir(writable_root, { recursive: true });
		await writeFile(path, bytes);
		return path;
	});

const Json = (body: unknown, status = 200) =>
	new Response(JSON.stringify(body), {
		headers: { "content-type": "application/json; charset=utf-8" },
		status,
	});

/**
 * Handles one tightly-scoped, same-origin desktop command without introducing
 * a preload or general renderer IPC bridge. Every accepted value names a
 * packaged asset; arbitrary paths and image bytes never cross the boundary.
 */
export const HandleDesktopAppIconRequest = (
	request: Request,
	controller: DesktopAppIconController,
) =>
	Effect.gen(function* () {
		const url = yield* Effect.try(() => new URL(request.url)).pipe(Effect.option);
		if (
			Option.isNone(url) ||
			url.value.host !== app_host ||
			url.value.pathname !== desktop_app_icon_control_path
		) {
			return Option.none<Response>();
		}

		if (request.method === "GET") {
			return Option.some(Json({ icon: controller.Current() }));
		}
		if (request.method !== "PUT") {
			return Option.some(new Response(null, { headers: { allow: "GET, PUT" }, status: 405 }));
		}

		const source = yield* Effect.tryPromise(() => request.text()).pipe(Effect.option);
		if (Option.isNone(source) || source.value.length > 256) {
			return Option.some(new Response(null, { status: 400 }));
		}
		const parsed = yield* Effect.try(() => JSON.parse(source.value) as unknown).pipe(
			Effect.option,
		);
		if (Option.isNone(parsed)) return Option.some(new Response(null, { status: 400 }));
		const selection = yield* Schema.decodeUnknownEffect(DesktopAppIconSelection)(
			parsed.value,
		).pipe(Effect.option);
		if (Option.isNone(selection)) {
			return Option.some(new Response(null, { status: 400 }));
		}

		const selected = yield* controller.Select(selection.value.icon).pipe(Effect.result);
		return Option.some(
			Result.isSuccess(selected)
				? new Response(null, { status: 204 })
				: new Response(null, { status: 500 }),
		);
	});

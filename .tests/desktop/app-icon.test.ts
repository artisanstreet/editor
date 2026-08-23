import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { default_desktop_app_icon, desktop_app_icon_control_path } from "@artisan/protocol";
import { Effect, Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	HandleDesktopAppIconRequest,
	LoadDesktopAppIconPreference,
	MaterializeWindowsAppIcon,
	SaveDesktopAppIconPreference,
} from "../../modules/desktop/src/app-icon";

describe("desktop app icon", () => {
	it("defaults malformed and missing preference files without blocking launch", async () => {
		expect(default_desktop_app_icon).toBe("foreground-gradient-symbol");
		const root = await mkdtemp(join(tmpdir(), "artisan-app-icon-"));
		const path = join(root, "app-icon.json");
		expect(await Effect.runPromise(LoadDesktopAppIconPreference(path))).toBe(
			default_desktop_app_icon,
		);
		await writeFile(path, '{"icon":"../../arbitrary.png","version":1}', "utf8");
		expect(await Effect.runPromise(LoadDesktopAppIconPreference(path))).toBe(
			default_desktop_app_icon,
		);
	});

	it("persists exactly one packaged icon id", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-app-icon-"));
		const path = join(root, "settings", "app-icon.json");
		await Effect.runPromise(SaveDesktopAppIconPreference(path, "foreground-gradient-symbol"));
		expect(await Effect.runPromise(LoadDesktopAppIconPreference(path))).toBe(
			"foreground-gradient-symbol",
		);
	});

	it("materializes a content-versioned ICO for the Windows shell", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-app-icon-"));
		const packaged = join(root, "packaged");
		const writable = join(root, "user-data", "app-icons");
		await mkdir(packaged, { recursive: true });
		const source = Buffer.from("windows-icon-bytes");
		await writeFile(join(packaged, "plastic-jaw-shading.ico"), source);

		const path = await Effect.runPromise(
			MaterializeWindowsAppIcon(packaged, writable, "plastic-jaw-shading"),
		);
		expect(path).toMatch(/plastic-jaw-shading-[a-f0-9]{12}\.ico$/u);
		expect(await readFile(path)).toEqual(source);
	});

	it("serves a narrow GET and PUT control surface without accepting asset paths", async () => {
		let current = default_desktop_app_icon;
		const controller = {
			Current: () => current,
			Select: (icon: typeof current) =>
				Effect.sync(() => {
					current = icon;
				}),
		};
		const endpoint = `artisan://app${desktop_app_icon_control_path}`;
		const Handle = (request: Request) =>
			Effect.runPromise(HandleDesktopAppIconRequest(request, controller));

		const read = Option.getOrThrow(await Handle(new Request(endpoint)));
		expect(read.status).toBe(200);
		expect(await read.json()).toEqual({ icon: default_desktop_app_icon });

		const selected = Option.getOrThrow(
			await Handle(
				new Request(endpoint, {
					body: JSON.stringify({ icon: "foreground-gradient-symbol" }),
					headers: { "content-type": "application/json" },
					method: "PUT",
				}),
			),
		);
		expect(selected.status).toBe(204);
		expect(current).toBe("foreground-gradient-symbol");

		const rejected = Option.getOrThrow(
			await Handle(
				new Request(endpoint, {
					body: JSON.stringify({ icon: "C:/arbitrary/icon.png" }),
					method: "PUT",
				}),
			),
		);
		expect(rejected.status).toBe(400);
		expect(current).toBe("foreground-gradient-symbol");

		expect(Option.isNone(await Handle(new Request("artisan://app/settings/appearance")))).toBe(
			true,
		);
	});
});

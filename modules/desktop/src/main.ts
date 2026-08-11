import { join, normalize } from "node:path";

import { BrowserWindow, app, protocol, session, shell } from "electron";
import { Effect, Layer } from "effect";

import {
	DesktopForgeLifecycle,
	DesktopRenderer,
	make_desktop_forge_lifecycle_layer,
	make_node_forge_handoff_process_layer,
} from "./forge-handoff";
import { resolve_desktop_paths } from "./paths";
import { app_host, app_scheme, ServeRendererAsset } from "./renderer-host";

const renderer_partition = "persist:artisan-renderer";
const windows_app_user_model_id = "com.usebarekey.artisan-editor";
const windows_toast_activator_clsid = "{A7D8D3E7-9DE2-4C09-8D4B-4E490C20D3A4}";

/** Windows toast delivery is keyed to the installed product identity. */
if (process.platform === "win32") {
	app.setAppUserModelId(windows_app_user_model_id);
	app.setToastActivatorCLSID(windows_toast_activator_clsid);
}

/**
 * Repairs launchers written before Artisan carried Windows toast identity.
 * The installer owns shortcut creation; the desktop host only upgrades the
 * existing Start Menu link in place so an installed update works immediately.
 */
const RepairWindowsNotificationShortcut = Effect.gen(function* () {
	if (process.platform !== "win32" || !app.isPackaged) return;

	const shortcut_path = join(
		app.getPath("appData"),
		"Microsoft",
		"Windows",
		"Start Menu",
		"Programs",
		"Artisan Editor.lnk",
	);

	yield* Effect.try({
		try: () => {
			const current = shell.readShortcutLink(shortcut_path);
			if (
				current.appUserModelId === windows_app_user_model_id &&
				current.toastActivatorClsid?.toUpperCase() ===
					windows_toast_activator_clsid.toUpperCase()
			) {
				return;
			}

			const updated = shell.writeShortcutLink(shortcut_path, "update", {
				...current,
				appUserModelId: windows_app_user_model_id,
				toastActivatorClsid: windows_toast_activator_clsid,
			});
			if (!updated) throw new Error("Electron declined the shortcut update");
		},
		catch: (cause) => cause,
	}).pipe(
		Effect.catch((cause) =>
			Effect.sync(() =>
				console.error(
					JSON.stringify({
						kind: "artisan:desktop-notification-shortcut",
						message: String(cause),
						ok: false,
					}),
				),
			),
		),
	);
});

/**
 * Loopback-only diagnosis hatch. The renderer has no IPC surface, so a memory
 * or rendering investigation on an installed build needs Chrome DevTools
 * Protocol access; setting the variable at launch is the only way in, and an
 * unset variable leaves no debugging listener at all.
 */
const remote_debugging_port = Number(process.env.ARTISAN_EDITOR_REMOTE_DEBUGGING_PORT ?? "");
if (
	Number.isInteger(remote_debugging_port) &&
	remote_debugging_port >= 1 &&
	remote_debugging_port <= 65_535
) {
	app.commandLine.appendSwitch("remote-debugging-port", String(remote_debugging_port));
	app.commandLine.appendSwitch("remote-debugging-address", "127.0.0.1");
}

/** Registration must precede app ready for `artisan://app` to be a real origin. */
protocol.registerSchemesAsPrivileged([
	{
		privileges: { secure: true, standard: true, supportFetchAPI: true },
		scheme: app_scheme,
	},
]);

/**
 * The installed editor remains a thin renderer host: `ae` owns Forge launch,
 * pairing, and exact-instance shutdown, while Electron retains only the lease
 * for a Forge this window caused `ae` to spawn. The renderer talks over plain
 * HTTP/WS like a paired browser; there is no preload or IPC surface.
 */
export const StartDesktop = Effect.gen(function* () {
	if (!app.requestSingleInstanceLock()) {
		app.quit();
		return;
	}

	yield* Effect.tryPromise({
		try: () => app.whenReady(),
		catch: (cause) => cause,
	});
	yield* RepairWindowsNotificationShortcut;
	const paths = resolve_desktop_paths({
		...(process.env.ARTISAN_AE_COMMAND === undefined
			? {}
			: { ae_command_override: process.env.ARTISAN_AE_COMMAND }),
		is_packaged: app.isPackaged,
		resources_path: process.resourcesPath,
	});
	const frontend_root = normalize(join(import.meta.dirname, "frontend"));
	/** Local renderer state survives restarts; short-lived Forge cookies do not. */
	const renderer_session = session.fromPartition(renderer_partition);
	renderer_session.protocol.handle(app_scheme, (request) =>
		Effect.runPromise(ServeRendererAsset(frontend_root, request.url)),
	);

	const editor_window = new BrowserWindow({
		autoHideMenuBar: true,
		backgroundColor: "#09090b",
		height: 900,
		minHeight: 480,
		minWidth: 720,
		show: false,
		title: "Artisan Editor",
		titleBarOverlay: { color: "#09090b", height: 40, symbolColor: "#9f9fa9" },
		titleBarStyle: "hidden",
		webPreferences: {
			contextIsolation: true,
			nodeIntegration: false,
			partition: renderer_partition,
			sandbox: true,
		},
		width: 1440,
	});
	editor_window.on("page-title-updated", (event) => event.preventDefault());
	editor_window.once("ready-to-show", () => editor_window.show());
	/** The renderer never opens child windows; external links go to the OS browser. */
	editor_window.webContents.setWindowOpenHandler(({ url }) => {
		if (url.startsWith("https:") || url.startsWith("http:")) void shell.openExternal(url);
		return { action: "deny" };
	});
	editor_window.webContents.on("will-navigate", (event, url) => {
		if (!url.startsWith(`${app_scheme}://${app_host}/`)) event.preventDefault();
	});

	const renderer = DesktopRenderer.of({
		ClearCookies: () =>
			Effect.tryPromise({
				try: () => renderer_session.clearStorageData({ storages: ["cookies"] }),
				catch: (cause) => cause,
			}),
		LoadUrl: (url) =>
			Effect.tryPromise({
				try: () => editor_window.loadURL(url),
				catch: (cause) => cause,
			}),
	});
	const desktop_lifecycle = yield* DesktopForgeLifecycle.pipe(
		Effect.provide(make_desktop_forge_lifecycle_layer(paths.ae_command_path)),
		Effect.provide(Layer.succeed(DesktopRenderer, renderer)),
		Effect.provide(make_node_forge_handoff_process_layer),
	);
	let cleanup: Promise<void> | undefined;
	const Cleanup = () => (cleanup ??= Effect.runPromise(desktop_lifecycle.Cleanup()));
	let quitting = false;
	app.on("before-quit", (event) => {
		if (quitting) return;
		event.preventDefault();
		void Cleanup().finally(() => {
			quitting = true;
			app.quit();
		});
	});
	app.on("second-instance", () => {
		if (editor_window.isMinimized()) editor_window.restore();
		editor_window.focus();
		/** A repeated `ae open` re-pairs the running editor with a fresh capability. */
		void Effect.runPromise(desktop_lifecycle.Reconnect());
	});
	app.on("window-all-closed", () => {
		app.quit();
	});

	yield* desktop_lifecycle.Start();
	/** The visible connection loader is already navigating while ae pairs in the background. */
	void Effect.runPromise(desktop_lifecycle.Reconnect());
});

/** The sole Desktop Effect runtime bootstrap. */
void Effect.runPromise(StartDesktop).catch((cause) => {
	console.error(
		JSON.stringify({
			kind: "artisan:desktop-launch",
			message: cause instanceof Error ? cause.message : "Unable to open the Artisan editor",
			ok: false,
		}),
	);
	app.exit(1);
});

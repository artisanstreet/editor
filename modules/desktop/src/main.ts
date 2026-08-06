import { spawn } from "node:child_process";
import { join, normalize } from "node:path";

import { BrowserWindow, app, protocol, session, shell } from "electron";
import { Effect } from "effect";

import { resolve_desktop_paths } from "./paths";
import {
	app_host,
	app_scheme,
	DecodeHandoffOutput,
	DesktopLauncherError,
	renderer_url,
	ServeRendererAsset,
	type ForgeHandoff,
} from "./renderer-host";

const renderer_partition = "artisan-renderer";

/**
 * Loopback-only diagnosis hatch. The renderer has no IPC surface, so a memory
 * or rendering investigation on an installed build needs Chrome DevTools
 * Protocol access; setting the variable at launch is the only way in, and an
 * unset variable leaves no debugging listener at all.
 */
const remote_debugging_port = process.env.ARTISAN_EDITOR_REMOTE_DEBUGGING_PORT;
if (remote_debugging_port !== undefined && /^\d{2,5}$/.test(remote_debugging_port)) {
	app.commandLine.appendSwitch("remote-debugging-port", remote_debugging_port);
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
 * Obtains `{endpoint, pair_code}` from the installed `ae`, which owns Forge
 * lifecycle and pairing for the home's single instance. The capability travels
 * only through this process's stdout pipe — never argv, disk, or a browser
 * navigation.
 */
const RequestForgeHandoff = (ae_command_path: string) =>
	Effect.callback<ForgeHandoff, DesktopLauncherError>((resume) => {
		const child = spawn(ae_command_path, ["open", "--handoff"], {
			shell: process.platform === "win32",
			stdio: ["ignore", "pipe", "ignore"],
			windowsHide: true,
		});
		let stdout = "";
		let settled = false;
		const complete = (result: Effect.Effect<ForgeHandoff, DesktopLauncherError>) => {
			if (settled) return;
			settled = true;
			child.off("error", on_error);
			child.off("exit", on_exit);
			resume(result);
		};
		const on_error = (cause: Error) =>
			complete(Effect.fail(new DesktopLauncherError({ cause, reason: "handoff_failed" })));
		const on_exit = (exit_code: number | null) =>
			complete(
				exit_code === 0
					? DecodeHandoffOutput(stdout)
					: Effect.fail(
							new DesktopLauncherError({
								cause: new Error(`ae open --handoff exited ${String(exit_code)}`),
								reason: "handoff_failed",
							}),
						),
			);
		child.stdout?.setEncoding("utf8");
		child.stdout?.on("data", (chunk: string) => {
			if (stdout.length < 64 * 1024) stdout += chunk;
		});
		child.once("error", on_error);
		child.once("exit", on_exit);

		return Effect.sync(() => {
			settled = true;
			child.off("error", on_error);
			child.off("exit", on_exit);
			child.kill();
		});
	});

/**
 * The installed editor is a windowed renderer host and nothing more: `ae`
 * owns Forge lifecycle and pairing; the renderer talks to Forge over plain
 * HTTP/WS exactly like a paired browser. There is no preload and no IPC
 * surface — the window is sandboxed with context isolation on.
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
	const paths = resolve_desktop_paths({
		...(process.env.ARTISAN_AE_COMMAND === undefined
			? {}
			: { ae_command_override: process.env.ARTISAN_AE_COMMAND }),
		is_packaged: app.isPackaged,
		resources_path: process.resourcesPath,
	});
	const frontend_root = normalize(join(import.meta.dirname, "frontend"));
	/**
	 * A non-persistent partition keeps the Forge session cookie in memory, so
	 * no credential outlives the window and every launch performs a fresh
	 * one-time pairing exchange.
	 */
	const renderer_session = session.fromPartition(renderer_partition);
	renderer_session.protocol.handle(app_scheme, (request) =>
		Effect.runPromise(ServeRendererAsset(frontend_root, request.url)),
	);

	const ObtainHandoff = Effect.suspend(() => RequestForgeHandoff(paths.ae_command_path)).pipe(
		Effect.tapCause((cause) =>
			Effect.sync(() =>
				console.error(
					JSON.stringify({
						kind: "artisan:desktop-handoff",
						message: String(cause),
						ok: false,
					}),
				),
			),
		),
		Effect.option,
	);

	const editor_window = new BrowserWindow({
		autoHideMenuBar: true,
		backgroundColor: "#09090b",
		height: 900,
		minHeight: 480,
		minWidth: 720,
		show: false,
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
	editor_window.once("ready-to-show", () => editor_window.show());
	/** The renderer never opens child windows; external links go to the OS browser. */
	editor_window.webContents.setWindowOpenHandler(({ url }) => {
		if (url.startsWith("https:") || url.startsWith("http:")) void shell.openExternal(url);
		return { action: "deny" };
	});
	editor_window.webContents.on("will-navigate", (event, url) => {
		if (!url.startsWith(`${app_scheme}://${app_host}/`)) event.preventDefault();
	});

	/**
	 * `artisan://forge/start` and repeated `ae open` invocations land here via
	 * the single-instance lock: the running editor re-pairs with a fresh
	 * one-time capability instead of spawning a second window.
	 */
	const LoadRenderer = Effect.gen(function* () {
		const handoff = yield* ObtainHandoff;
		yield* Effect.tryPromise({
			try: () => editor_window.loadURL(renderer_url(handoff)),
			catch: (cause) => cause,
		});
	});
	app.on("second-instance", () => {
		if (editor_window.isMinimized()) editor_window.restore();
		editor_window.focus();
		/** A repeated `ae open` re-pairs the running editor with a fresh capability. */
		void Effect.runPromise(LoadRenderer).catch(() => undefined);
	});
	app.on("window-all-closed", () => app.quit());

	yield* LoadRenderer;
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

import { app, BrowserWindow, MessageChannelMain, protocol, utilityProcess } from "electron";
import { readFile } from "node:fs/promises";
import { delimiter, dirname, extname, join } from "node:path";

import { resolve_desktop_paths } from "./paths";
import { read_desktop_identity } from "./identity";
import { DesktopRendererOrigin, resolve_frontend_request } from "./protocol";
import { DesktopSessionSupervisor } from "./session-supervisor";
import { RunPackagedDesktopSmoke } from "./packaged-smoke";
import { make_desktop_window_activity } from "./window-activity";

const request_channel = "artisan:request-connection";
const identity_channel = "artisan:desktop-identity";
const activity_channel = "artisan:desktop-activity";
let main_window: BrowserWindow | undefined;
let quitting = false;

if (process.env.ARTISAN_PACKAGED_SMOKE === "1") {
	const smoke_user_data = process.env.ARTISAN_PACKAGED_SMOKE_USER_DATA;
	if (!smoke_user_data) {
		throw new Error("Packaged desktop smoke requires an isolated user-data directory");
	}
	app.setPath("userData", smoke_user_data);
}

protocol.registerSchemesAsPrivileged([
	{
		privileges: { codeCache: true, secure: true, standard: true, supportFetchAPI: true },
		scheme: "artisan",
	},
]);

const allowed_url = (url: string) =>
	url === `${DesktopRendererOrigin}/` || url.startsWith(`${DesktopRendererOrigin}/`);

const frontend_content_type = (file: string) => {
	switch (extname(file).toLowerCase()) {
		case ".css":
			return "text/css";
		case ".html":
			return "text/html";
		case ".js":
			return "text/javascript";
		case ".json":
			return "application/json";
		case ".svg":
			return "image/svg+xml";
		case ".woff":
			return "font/woff";
		case ".woff2":
			return "font/woff2";
		default:
			return "application/octet-stream";
	}
};

/** Starts exactly one hardened shell window and its dedicated backend utility process. */
export const StartDesktop = async () => {
	if (!app.requestSingleInstanceLock()) {
		if (process.env.ARTISAN_PACKAGED_SMOKE === "1") app.exit(1);
		else app.quit();
		return;
	}

	await app.whenReady();
	const paths = resolve_desktop_paths({
		app_data_path: app.getPath("userData"),
		app_root_path: app.getAppPath(),
		resources_path: process.resourcesPath,
	});
	const supervisor = new DesktopSessionSupervisor({
		create_channel: () => new MessageChannelMain(),
		fork_utility: (utility_path) =>
			utilityProcess.fork(utility_path, [], {
				env: {
					...process.env,
					ARTISAN_DATABASE_PATH: paths.database_path,
					ARTISAN_MIGRATIONS_PATH: paths.migrations_path,
					NODE_PATH:
						process.env.ARTISAN_PACKAGED_SMOKE === "1"
							? join(dirname(utility_path), "native-runtime")
							: [join(dirname(utility_path), "native-runtime"), process.env.NODE_PATH]
									.filter(Boolean)
									.join(delimiter),
				},
				serviceName: "artisan-backend",
				// Release smoke inherits utility diagnostics into its redirected gate logs.
				// Normal desktop sessions keep the backend process detached from a console.
				stdio: process.env.ARTISAN_PACKAGED_SMOKE === "1" ? "inherit" : "ignore",
			}) as never,
		paths,
		report_diagnostic: (message) => {
			if (process.env.ARTISAN_PACKAGED_SMOKE === "1") {
				console.error(JSON.stringify(message));
			}
		},
		schedule: (callback, milliseconds) => setTimeout(callback, milliseconds),
	});
	const desktop_identity = read_desktop_identity();
	await protocol.handle("artisan", async (request) => {
		const file = resolve_frontend_request(paths.frontend_root, request.url);

		if (!file) {
			return new Response(
				JSON.stringify({ frontend_root: paths.frontend_root, request_url: request.url }),
				{ status: 404 },
			);
		}
		try {
			return new Response(await readFile(file), {
				headers: { "content-type": frontend_content_type(file) },
			});
		} catch (cause) {
			return new Response(
				JSON.stringify({
					file,
					message:
						cause instanceof Error ? cause.message : "Unable to read packaged asset",
				}),
				{ status: 404 },
			);
		}
	});
	const CreateWindow = async () => {
		if (main_window) {
			return main_window;
		}

		const window = new BrowserWindow({
			webPreferences: {
				contextIsolation: true,
				nodeIntegration: false,
				preload: paths.preload_path,
				sandbox: true,
			},
		});

		main_window = window;
		const activity = make_desktop_window_activity(window);
		/** Frame-scoped IPC accepts messages only from this top-level renderer frame. */
		window.webContents.mainFrame.ipc.handle(request_channel, (event) =>
			supervisor.RequestConnection(event as never),
		);
		window.webContents.mainFrame.ipc.handle(identity_channel, () => desktop_identity);
		window.webContents.mainFrame.ipc.handle(activity_channel, (_event, working: unknown) => {
			if (typeof working !== "boolean") {
				throw new Error("Desktop activity state must be a boolean");
			}
			activity.SetWorking(working);
		});
		window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
		window.webContents.on("will-navigate", (event, url) => {
			if (!allowed_url(url)) {
				event.preventDefault();
			}
		});
		window.on("closed", () => {
			activity.RestoreIdle();
			if (main_window === window) {
				main_window = undefined;
			}
		});
		await window.loadURL(`${DesktopRendererOrigin}/`);

		return window;
	};
	app.on("before-quit", (event) => {
		if (quitting) {
			return;
		}

		event.preventDefault();
		void supervisor.Dispose().finally(() => {
			quitting = true;
			app.quit();
		});
	});
	app.on("second-instance", () => {
		if (main_window?.isMinimized()) {
			main_window.restore();
		}
		main_window?.focus();
	});
	app.on("activate", () => {
		if (!quitting) {
			void CreateWindow();
		}
	});
	app.on("window-all-closed", () => {
		if (process.platform !== "darwin") {
			app.quit();
		}
	});

	supervisor.Start();
	if (process.env.ARTISAN_PACKAGED_SMOKE === "1") {
		let exit_code = 0;
		try {
			const renderer = await CreateWindow();
			const evidence = await RunPackagedDesktopSmoke({ renderer, supervisor });
			console.log(JSON.stringify({ kind: "artisan:packaged-smoke", ok: true, ...evidence }));
		} catch (cause) {
			console.error(
				JSON.stringify({
					kind: "artisan:packaged-smoke",
					message:
						cause instanceof Error ? cause.message : "Unknown packaged smoke failure",
					ok: false,
				}),
			);
			exit_code = 1;
		} finally {
			try {
				main_window?.destroy();
			} catch {
				/** Renderer teardown is best-effort before the process-level smoke exit. */
			}
			try {
				await supervisor.Dispose();
			} catch {
				/** The smoke already has its own failure result; shutdown must not prevent exit. */
			} finally {
				app.exit(exit_code);
			}
		}
		return;
	}
	await CreateWindow();
};

void StartDesktop().catch((cause) => {
	console.error(
		JSON.stringify({
			kind: "artisan:packaged-smoke",
			message: cause instanceof Error ? cause.message : "Desktop startup failed",
			ok: false,
		}),
	);
	app.exit(1);
});

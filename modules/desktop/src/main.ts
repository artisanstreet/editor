import {
	app,
	BrowserWindow,
	ipcMain,
	MessageChannelMain,
	net,
	protocol,
	utilityProcess,
} from "electron";
import { pathToFileURL } from "node:url";

import { resolve_desktop_paths } from "./paths";
import { DesktopRendererOrigin, resolve_frontend_request } from "./protocol";
import { DesktopSessionSupervisor } from "./session-supervisor";

const request_channel = "artisan:request-connection";
let main_window: BrowserWindow | undefined;
let quitting = false;

protocol.registerSchemesAsPrivileged([
	{
		privileges: { codeCache: true, secure: true, standard: true, supportFetchAPI: true },
		scheme: "artisan",
	},
]);

const allowed_url = (url: string) => url === `${DesktopRendererOrigin}/` || url.startsWith(`${DesktopRendererOrigin}/`);

/** Starts exactly one hardened shell window and its dedicated backend utility process. */
export const StartDesktop = async () => {
	if (!app.requestSingleInstanceLock()) {
		app.quit();
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
					ARTISAN_DATABASE_PATH: paths.database_path,
					ARTISAN_MIGRATIONS_PATH: paths.migrations_path,
				},
				serviceName: "artisan-backend",
				stdio: "ignore",
			}) as never,
		paths,
		schedule: (callback, milliseconds) => setTimeout(callback, milliseconds),
	});
	protocol.handle("artisan", (request) => {
		const file = resolve_frontend_request(paths.frontend_root, request.url);

		return file
			? net.fetch(pathToFileURL(file).toString())
			: new Response("Not found", { status: 404 });
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
		window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
		window.webContents.on("will-navigate", (event, url) => {
			if (!allowed_url(url)) {
				event.preventDefault();
			}
		});
		window.on("closed", () => {
			if (main_window === window) {
				main_window = undefined;
			}
		});
		await window.loadURL(`${DesktopRendererOrigin}/index.html`);

		return window;
	};
	ipcMain.handle(request_channel, (event) => {
		if (event.sender !== main_window?.webContents || event.frameId !== event.sender.mainFrame.routingId) {
			throw new Error("Only the primary Artisan renderer may request a backend connection");
		}

		return supervisor.RequestConnection(event as never);
	});
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
	await CreateWindow();
};

void StartDesktop();

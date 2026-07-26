import { app, BrowserWindow, dialog, Menu, session as electron_session } from "electron";
import { Deferred, Duration, Effect, Option, Queue, Ref, Schedule } from "effect";

import { AcquireForgeProcessSupervisor } from "./forge-process-supervisor";
import { read_desktop_identity } from "./identity";
import { resolve_desktop_paths } from "./paths";
import { SelectDesktopProjectDirectory } from "./project-picker";
import { make_desktop_window_activity } from "./window-activity";

const identity_channel = "artisan:desktop-identity";
const activity_channel = "artisan:desktop-activity";
const project_picker_channel = "artisan:select-project-directory";

if (process.env.ARTISAN_PACKAGED_SMOKE === "1") {
	app.commandLine.appendSwitch("in-process-gpu");
	app.commandLine.appendSwitch("no-sandbox");
	const smoke_user_data = process.env.ARTISAN_PACKAGED_SMOKE_USER_DATA;
	if (!smoke_user_data) {
		throw new Error("Packaged desktop smoke requires an isolated user-data directory");
	}
	app.setPath("userData", smoke_user_data);
}

const AwaitAppReady = Effect.tryPromise({
	try: () => app.whenReady(),
	catch: (cause) => cause,
});

const InstallBrowserSession = ({
	http_endpoint,
	token,
}: {
	readonly http_endpoint: string;
	readonly token: string;
}) =>
	Effect.tryPromise({
		try: () =>
			electron_session.defaultSession.cookies.set({
				httpOnly: true,
				name: "artisan_forge_session",
				path: "/",
				sameSite: "strict",
				secure: http_endpoint.startsWith("https:"),
				url: http_endpoint,
				value: token,
			}),
		catch: (cause) => cause,
	});

/** Runs the complete Electron shell lifecycle inside one Effect scope. */
export const StartDesktop = Effect.scoped(
	Effect.gen(function* () {
		if (!app.requestSingleInstanceLock()) {
			if (process.env.ARTISAN_PACKAGED_SMOKE === "1") app.exit(1);
			else app.quit();
			return;
		}

		yield* AwaitAppReady;
		Menu.setApplicationMenu(null);
		const paths = resolve_desktop_paths({
			app_data_path: app.getPath("userData"),
			app_root_path: app.getAppPath(),
			is_packaged: app.isPackaged,
			resources_path: process.resourcesPath,
		});
		const supervisor = yield* AcquireForgeProcessSupervisor(paths, {
			InstallBrowserSession,
		});
		const forge_connection = yield* supervisor.Start;
		const forge_http_endpoint = new URL(forge_connection.http_endpoint);
		const renderer_origin = forge_http_endpoint.origin;
		const desktop_identity = yield* read_desktop_identity;
		const main_window = yield* Ref.make<Option.Option<BrowserWindow>>(Option.none());
		const quitting = yield* Ref.make(false);
		const shutdown = yield* Deferred.make<void>();
		const dispatch_queue = yield* Queue.unbounded<{
			readonly effect: Effect.Effect<unknown, unknown>;
			readonly reject: (cause: unknown) => void;
			readonly resolve: (value: unknown) => void;
		}>();
		yield* Effect.forkScoped(
			Effect.forever(
				Queue.take(dispatch_queue).pipe(
					Effect.flatMap((request) =>
						request.effect.pipe(
							Effect.match({
								onFailure: (cause) => request.reject(cause),
								onSuccess: (value) => request.resolve(value),
							}),
						),
					),
				),
			),
		);
		const DispatchPromise = <A, E>(effect: Effect.Effect<A, E>) =>
			new Promise<A>((resolve, reject) => {
				const accepted = Queue.offerUnsafe(dispatch_queue, {
					effect,
					reject,
					resolve: (value) => resolve(value as A),
				});
				if (!accepted) reject(new Error("Desktop Effect dispatcher is closed"));
			});
		const Dispatch = <A, E>(effect: Effect.Effect<A, E>) => {
			void DispatchPromise(effect).catch((cause) => console.error(cause));
		};

		const CreateWindow = Effect.gen(function* () {
			const current = yield* Ref.get(main_window);
			if (Option.isSome(current)) return current.value;

			const window = new BrowserWindow({
				...(process.platform === "darwin"
					? {
							titleBarStyle: "hiddenInset" as const,
							trafficLightPosition: { x: 16, y: 16 },
						}
					: {
							titleBarStyle: "hidden" as const,
							titleBarOverlay: {
								color: "#00000000",
								height: 40,
								symbolColor: "#a1a1aa",
							},
						}),
				webPreferences: {
					additionalArguments: [
						`--artisan-forge-ws=${forge_connection.websocket_endpoint}`,
					],
					contextIsolation: true,
					nodeIntegration: false,
					preload: paths.preload_path,
					sandbox: true,
				},
			});
			yield* Ref.set(main_window, Option.some(window));
			const activity = yield* make_desktop_window_activity(window);
			window.webContents.mainFrame.ipc.handle(identity_channel, () => desktop_identity);
			window.webContents.mainFrame.ipc.handle(activity_channel, (_event, working: unknown) =>
				DispatchPromise(
					typeof working === "boolean"
						? activity.SetWorking(working)
						: Effect.fail(new Error("Desktop activity state must be a boolean")),
				),
			);
			window.webContents.mainFrame.ipc.handle(project_picker_channel, () =>
				DispatchPromise(
					SelectDesktopProjectDirectory({
						ShowOpenDialog: () =>
							Effect.tryPromise({
								try: () =>
									dialog.showOpenDialog(window, {
										buttonLabel: "Choose project",
										properties: ["createDirectory", "openDirectory"],
										title: "Choose project folder",
									}),
								catch: (cause) => cause,
							}),
					}).pipe(Effect.map(Option.getOrUndefined)),
				),
			);
			window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
			window.webContents.on("will-navigate", (event, url) => {
				if (new URL(url).origin !== renderer_origin) event.preventDefault();
			});
			window.on("closed", () => {
				Dispatch(
					activity.RestoreIdle.pipe(
						Effect.andThen(
							Ref.update(main_window, (candidate) =>
								Option.filter(candidate, (value) => value !== window),
							),
						),
					),
				);
			});
			yield* Effect.tryPromise({
				try: () => window.loadURL(forge_http_endpoint.toString()),
				catch: (cause) => cause,
			});
			return window;
		});

		const on_before_quit = (event: Electron.Event) => {
			event.preventDefault();
			Dispatch(
				Ref.modify(quitting, (current) => [!current, true] as const).pipe(
					Effect.flatMap((first_request) =>
						first_request
							? supervisor.Dispose.pipe(
									Effect.andThen(Deferred.succeed(shutdown, undefined)),
								)
							: Effect.void,
					),
				),
			);
		};
		const on_second_instance = () => {
			Dispatch(
				Ref.get(main_window).pipe(
					Effect.tap((candidate) =>
						Effect.sync(() => {
							const window = Option.getOrUndefined(candidate);
							if (window?.isMinimized()) window.restore();
							window?.focus();
						}),
					),
				),
			);
		};
		const on_activate = () => {
			Dispatch(
				Ref.get(quitting).pipe(
					Effect.flatMap((is_quitting) => (is_quitting ? Effect.void : CreateWindow)),
				),
			);
		};
		const on_all_closed = () => {
			if (process.platform !== "darwin") app.quit();
		};
		app.on("before-quit", on_before_quit);
		app.on("second-instance", on_second_instance);
		app.on("activate", on_activate);
		app.on("window-all-closed", on_all_closed);
		yield* Effect.addFinalizer(() =>
			Effect.sync(() => {
				app.off("before-quit", on_before_quit);
				app.off("second-instance", on_second_instance);
				app.off("activate", on_activate);
				app.off("window-all-closed", on_all_closed);
			}),
		);

		const window = yield* CreateWindow;
		if (process.env.ARTISAN_PACKAGED_SMOKE === "1") {
			const ReadRendererEvidence = Effect.tryPromise({
				try: async () => {
					const evidence = await window.webContents.executeJavaScript(
						`(() => {
							const body = document.body?.innerText ?? "";
							return document.title === "Artisan Editor" && body.includes("No threads yet. Create one from the sidebar.")
								? { body, has_native_bridge: typeof window.artisanDesktop?.identity === "function", title: document.title }
								: undefined;
						})()`,
						true,
					);
					if (evidence === undefined) throw new Error("Renderer is not ready");
					return evidence;
				},
				catch: (cause) => cause,
			}).pipe(
				Effect.retry(Schedule.spaced(Duration.millis(50))),
				Effect.timeoutOrElse({
					duration: Duration.seconds(20),
					orElse: () => Effect.fail(new Error("Packaged renderer readiness timed out")),
				}),
			);
			const renderer_evidence = yield* ReadRendererEvidence;
			const forge_pid = Option.getOrUndefined(yield* supervisor.GetForgePid);
			console.log(
				JSON.stringify({
					forge_pid,
					forge_websocket_endpoint: forge_connection.websocket_endpoint,
					kind: "artisan:packaged-smoke",
					ok: renderer_evidence !== undefined && forge_pid !== undefined,
					renderer: renderer_evidence,
				}),
			);
			app.exit(renderer_evidence !== undefined && forge_pid !== undefined ? 0 : 1);
			return;
		}
		yield* Deferred.await(shutdown);
		app.off("before-quit", on_before_quit);
		app.quit();
	}),
);

/** The sole Desktop Effect runtime bootstrap. */
void Effect.runPromise(StartDesktop).catch((cause) => {
	console.error(
		JSON.stringify({
			kind: "artisan:packaged-smoke",
			message: cause instanceof Error ? cause.message : "Desktop startup failed",
			ok: false,
		}),
	);
	app.exit(1);
});

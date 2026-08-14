import { join, normalize } from "node:path";

import { AttentionCountFromTitle } from "@artisan/protocol";
import { BrowserWindow, app, contentTracing, protocol, session, shell } from "electron";
import { Effect, Layer, Option } from "effect";

import { AttentionOverlayDescription } from "./badge/catalog";
import { AttentionOverlayImage } from "./badge/overlay";
import {
	DesktopForgeLifecycle,
	DesktopRenderer,
	make_desktop_forge_lifecycle_layer,
	make_node_forge_handoff_process_layer,
} from "./forge-handoff";
import { resolve_desktop_paths } from "./paths";
import { CreateRendererDiagnosticLog } from "./renderer-diagnostics";
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
const renderer_diagnostics_enabled = process.env.ARTISAN_EDITOR_RENDERER_DIAGNOSTICS === "1";
const trace_renderer_freezes = process.env.ARTISAN_EDITOR_TRACE_FREEZES === "1";
const open_dev_tools = process.env.ARTISAN_EDITOR_OPEN_DEVTOOLS === "1";
const renderer_heartbeat_interval_ms = 500;
const renderer_heartbeat_timeout_ms = 1_250;
const renderer_metrics_interval_ms = 5_000;
const trace_capture_cooldown_ms = 20_000;
const trace_capture_timeout_ms = 15_000;
const trace_buffer_size_in_kb = 64 * 1024;
const dev_tools_trace_categories = [
	"electron",
	"toplevel",
	"devtools.timeline",
	"disabled-by-default-devtools.timeline",
	"disabled-by-default-devtools.timeline.frame",
	"disabled-by-default-devtools.timeline.stack",
	"disabled-by-default-devtools.timeline.invalidationTracking",
	"blink",
	"blink.user_timing",
	"renderer.scheduler",
	"v8",
	"v8.execute",
	"disabled-by-default-v8.cpu_profiler",
	"disabled-by-default-v8.cpu_profiler.hires",
	"cc",
	"gpu",
	"input",
	"latencyInfo",
].join(",");
if (
	Number.isInteger(remote_debugging_port) &&
	remote_debugging_port >= 1 &&
	remote_debugging_port <= 65_535
) {
	app.commandLine.appendSwitch("remote-debugging-port", String(remote_debugging_port));
	app.commandLine.appendSwitch("remote-debugging-address", "127.0.0.1");
}
if (renderer_diagnostics_enabled) {
	app.commandLine.appendSwitch("enable-logging", "stderr");
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
	/**
	 * The renderer owns the document title; the window keeps its fixed name.
	 * That pinned title is also the shell's only renderer-owned signal — there
	 * is no preload and no IPC — so the needs-attention marker the renderer
	 * embeds in it becomes the taskbar badge here: a numbered overlay where
	 * Windows wants an image, the native dock badge everywhere else.
	 */
	editor_window.on("page-title-updated", (event, title) => {
		event.preventDefault();
		const attention_count = AttentionCountFromTitle(title) ?? 0;
		if (process.platform === "win32") {
			if (attention_count === 0) {
				editor_window.setOverlayIcon(null, "");
			} else {
				editor_window.setOverlayIcon(
					AttentionOverlayImage(attention_count),
					AttentionOverlayDescription(attention_count),
				);
			}
			return;
		}
		app.setBadgeCount(attention_count);
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
	let cleanup_renderer_diagnostics = () => undefined;
	if (renderer_diagnostics_enabled) {
		const diagnostics_root =
			process.env.ARTISAN_DIAGNOSTICS_DIR ?? join(app.getPath("logs"), "diagnostics");
		const session_name = `${new Date().toISOString().replaceAll(":", "-")}-${process.pid}`;
		const diagnostic_log = yield* CreateRendererDiagnosticLog(
			join(diagnostics_root, "renderer"),
			session_name,
		).pipe(Effect.option);
		let unresponsive_at: number | undefined;
		const renderer_process_id = () => editor_window.webContents.getOSProcessId();
		const app_metrics = () => app.getAppMetrics();
		const renderer_metrics = () =>
			app_metrics().find((metric) => metric.pid === renderer_process_id());
		const log_renderer_diagnostic = (details: Record<string, unknown>) => {
			const record = {
				...details,
				kind: "artisan:renderer-diagnostics",
				pid: renderer_process_id(),
				timestamp: new Date().toISOString(),
			};
			console.error(JSON.stringify(record));
			Option.match(diagnostic_log, {
				onNone: () => undefined,
				onSome: (log) => void Effect.runPromise(log.Write(record).pipe(Effect.ignore)),
			});
		};
		log_renderer_diagnostic({
			app_version: app.getVersion(),
			electron_version: process.versions.electron,
			event: "session-started",
			log_path: Option.getOrUndefined(diagnostic_log)?.path,
		});
		const LogProcessMetrics = () =>
			void Effect.runPromise(
				Effect.tryPromise({
					try: () => process.getProcessMemoryInfo(),
					catch: (cause) => cause,
				}).pipe(
					Effect.match({
						onFailure: (cause) =>
							Effect.sync(() =>
								log_renderer_diagnostic({
									event: "process-metrics-failed",
									message: String(cause),
								}),
							),
						onSuccess: (main_memory) =>
							Effect.sync(() =>
								log_renderer_diagnostic({
									event: "process-metrics",
									app_metrics: app_metrics(),
									main_cpu_usage: process.getCPUUsage(),
									main_memory,
									renderer_metrics: renderer_metrics(),
								}),
							),
					}),
				),
			);
		let trace_capture_in_progress = false;
		let trace_capture_sequence = 0;
		let last_trace_capture_at = Number.NEGATIVE_INFINITY;
		const StartFreezeTrace = () =>
			Effect.tryPromise({
				try: () =>
					contentTracing.startRecording({
						included_categories: dev_tools_trace_categories.split(","),
						recording_mode: "record-continuously",
						trace_buffer_size_in_kb,
					}),
				catch: (cause) => cause,
			});
		const CaptureFreezeTrace = (trigger: string) => {
			const log = Option.getOrUndefined(diagnostic_log);
			if (!trace_renderer_freezes || trace_capture_in_progress || log === undefined) return;
			if (Date.now() - last_trace_capture_at < trace_capture_cooldown_ms) {
				log_renderer_diagnostic({
					event: "trace-capture-coalesced",
					trigger,
				});
				return;
			}
			trace_capture_in_progress = true;
			last_trace_capture_at = Date.now();
			trace_capture_sequence += 1;
			LogProcessMetrics();
			const trace_path = log.path.replace(
				/\.jsonl$/u,
				`.trace-${String(trace_capture_sequence).padStart(3, "0")}.json`,
			);
			let capture_timed_out = false;
			const timeout = setTimeout(() => {
				capture_timed_out = true;
				log_renderer_diagnostic({
					event: "trace-capture-timeout",
					trace_path,
					trigger,
				});
			}, trace_capture_timeout_ms);
			void Effect.runPromise(
				Effect.gen(function* () {
					const trace_buffer_before = yield* Effect.tryPromise({
						try: () => contentTracing.getTraceBufferUsage(),
						catch: (cause) => cause,
					}).pipe(Effect.option);
					log_renderer_diagnostic({
						event: "trace-capture-started",
						trace_buffer_usage: Option.getOrUndefined(trace_buffer_before),
						trace_path,
						trigger,
					});
					yield* Effect.tryPromise({
						try: () => contentTracing.stopRecording(trace_path),
						catch: (cause) => cause,
					});
					const trace_buffer_after = yield* Effect.tryPromise({
						try: () => contentTracing.getTraceBufferUsage(),
						catch: (cause) => cause,
					}).pipe(Effect.option);
					log_renderer_diagnostic({
						event: capture_timed_out ? "trace-captured-late" : "trace-captured",
						trace_buffer_usage: Option.getOrUndefined(trace_buffer_after),
						trace_path,
						trigger,
					});
					LogProcessMetrics();
					yield* StartFreezeTrace();
				}).pipe(
					Effect.catch((cause) =>
						Effect.sync(() =>
							log_renderer_diagnostic({
								event: "trace-capture-failed",
								message: String(cause),
							}),
						),
					),
					Effect.ensuring(
						Effect.sync(() => {
							clearTimeout(timeout);
							trace_capture_in_progress = false;
						}),
					),
				),
			);
		};
		if (trace_renderer_freezes && Option.isSome(diagnostic_log)) {
			yield* StartFreezeTrace().pipe(
				Effect.catch((cause) =>
					Effect.sync(() =>
						log_renderer_diagnostic({
							event: "trace-start-failed",
							message: String(cause),
						}),
					),
				),
			);
		}
		let renderer_loaded = false;
		let heartbeat_in_progress = false;
		let heartbeat_generation = 0;
		const RunRendererHeartbeat = () => {
			if (
				!renderer_loaded ||
				heartbeat_in_progress ||
				editor_window.webContents.isDestroyed()
			)
				return;
			heartbeat_in_progress = true;
			const generation = heartbeat_generation;
			const started_at = Date.now();
			let reported_stall = false;
			const timeout = setTimeout(() => {
				if (generation !== heartbeat_generation) return;
				reported_stall = true;
				log_renderer_diagnostic({
					duration_ms: Date.now() - started_at,
					event: "heartbeat-stalled",
					metrics: renderer_metrics(),
				});
				CaptureFreezeTrace("heartbeat-stalled");
			}, renderer_heartbeat_timeout_ms);
			void editor_window.webContents.executeJavaScript("void 0").then(
				() => {
					clearTimeout(timeout);
					if (generation !== heartbeat_generation) return;
					heartbeat_in_progress = false;
					const duration_ms = Date.now() - started_at;
					if (reported_stall) {
						log_renderer_diagnostic({ duration_ms, event: "heartbeat-recovered" });
					} else if (duration_ms >= renderer_heartbeat_interval_ms) {
						log_renderer_diagnostic({ duration_ms, event: "heartbeat-slow" });
					}
				},
				(cause: unknown) => {
					clearTimeout(timeout);
					if (generation !== heartbeat_generation) return;
					heartbeat_in_progress = false;
					log_renderer_diagnostic({ event: "heartbeat-failed", message: String(cause) });
				},
			);
		};
		const heartbeat_interval = setInterval(
			RunRendererHeartbeat,
			renderer_heartbeat_interval_ms,
		);
		const metrics_interval = setInterval(LogProcessMetrics, renderer_metrics_interval_ms);
		editor_window.webContents.on("did-finish-load", () => {
			renderer_loaded = true;
			RunRendererHeartbeat();
			LogProcessMetrics();
		});
		editor_window.webContents.on("did-start-loading", () => {
			renderer_loaded = false;
			heartbeat_generation += 1;
			heartbeat_in_progress = false;
		});

		editor_window.webContents.on("unresponsive", () => {
			if (unresponsive_at !== undefined) return;
			unresponsive_at = Date.now();
			log_renderer_diagnostic({ event: "unresponsive", metrics: renderer_metrics() });
			CaptureFreezeTrace("unresponsive");
		});
		editor_window.webContents.on("responsive", () => {
			if (unresponsive_at === undefined) return;
			const duration_ms = Date.now() - unresponsive_at;
			unresponsive_at = undefined;
			log_renderer_diagnostic({ event: "responsive", duration_ms });
		});
		editor_window.webContents.on("render-process-gone", (_event, details) => {
			renderer_loaded = false;
			heartbeat_generation += 1;
			heartbeat_in_progress = false;
			log_renderer_diagnostic({
				event: "render-process-gone",
				exit_code: details.exitCode,
				reason: details.reason,
			});
			CaptureFreezeTrace("render-process-gone");
		});
		/** Captures a post-hitch trace without exposing any renderer IPC surface. */
		editor_window.webContents.on("before-input-event", (event, input) => {
			if (input.type !== "keyDown" || !input.control || !input.shift || input.key !== "F9")
				return;
			event.preventDefault();
			log_renderer_diagnostic({ event: "trace-capture-requested", trigger: "shortcut" });
			CaptureFreezeTrace("shortcut");
		});
		cleanup_renderer_diagnostics = () => {
			clearInterval(heartbeat_interval);
			clearInterval(metrics_interval);
			renderer_loaded = false;
			heartbeat_generation += 1;
			if (trace_renderer_freezes && !trace_capture_in_progress) {
				void contentTracing.stopRecording().catch(() => undefined);
			}
		};
	}
	if (open_dev_tools) {
		editor_window.webContents.once("did-finish-load", () => {
			editor_window.webContents.openDevTools({ mode: "detach" });
		});
	}

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
			cleanup_renderer_diagnostics();
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

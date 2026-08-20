import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { resolve_desktop_paths } from "@artisan/desktop";

const root = new URL("../..", import.meta.url);

describe("desktop packaging configuration", () => {
	it("uses explicit mutable and packaged paths", () => {
		expect(
			resolve_desktop_paths({
				executable_path: "C:/artisan/editor",
				resources_path: "C:/resources",
			}),
		).toMatchObject({
			ae_command_path: "ae",
		});
	});

	it("resolves the packaged daemon independently from the Electron application archive", () => {
		expect(
			resolve_desktop_paths({
				executable_path:
					"C:\\Users\\test\\AppData\\Local\\Artisan\\versions\\0.2.61\\editor\\Artisan Editor.exe",
				is_packaged: true,
				resources_path: "C:/resources",
			}),
		).toMatchObject({
			ae_command_path: "C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe",
		});
	});

	it("prefers the managed permanent ae supplied by the distribution launcher", () => {
		expect(
			resolve_desktop_paths({
				ae_command_override: "C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.cmd",
				executable_path:
					"C:\\Users\\test\\AppData\\Local\\Artisan\\versions\\0.2.61\\editor\\Artisan Editor.exe",
				is_packaged: true,
				resources_path: "C:/resources",
			}),
		).toEqual({
			ae_command_path: "C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.cmd",
		});
	});

	it("packages a sandboxed renderer host without any privileged bridge", () => {
		const config = readFileSync(new URL(".config/desktop-builder.yml", root), "utf8");
		const package_manifest = JSON.parse(
			readFileSync(new URL("package.json", root), "utf8"),
		) as { readonly scripts?: Record<string, string> };
		const main = readFileSync(new URL("modules/desktop/src/main.ts", root), "utf8");
		const vite_config = readFileSync(new URL(".config/desktop.vite.config.ts", root), "utf8");

		expect(config).toContain("output: .dist/electron-release");
		expect(config).toContain("app: .dist/desktop");
		expect(config).toContain("electronLanguages: en-US");
		expect(config).toContain("- main.js");
		expect(config).toContain("- frontend/**");
		expect(config).not.toContain("preload.cjs");
		expect(config).not.toContain("asarUnpack:");
		expect(config).not.toContain("extraResources:");
		expect(config).not.toContain("from: .dist/forge");
		expect(config).not.toContain("nsis");
		expect(config).not.toContain("signExecutable: false");
		expect(config).toContain("- dir");
		expect(config).toContain("name: Artisan Forge");
		expect(config).toContain("- artisan");
		expect(config).not.toContain("from: .dist\n");
		expect(package_manifest.scripts?.["build:desktop"]).not.toContain("preload");
		expect(vite_config).not.toContain("node-pty");
		expect(vite_config).not.toContain("koffi");
		/** The staged renderer copy gets the loopback CSP variant; the browser copy stays same-origin. */
		expect(vite_config).toContain("connect-src 'self' http://127.0.0.1:*");
		expect(main).toContain("requestSingleInstanceLock");
		expect(main).not.toContain("setAsDefaultProtocolClient");
		/** The window is a pure renderer host: sandboxed, isolated, and bridge-free. */
		expect(main).toContain("BrowserWindow");
		expect(main).toContain("contextIsolation: true");
		expect(main).toContain("nodeIntegration: false");
		expect(main).toContain("sandbox: true");
		/** A backgrounded window must keep the transport heartbeat alive. */
		expect(main).toContain("backgroundThrottling: false");
		expect(main).toContain('title: "Artisan Editor"');
		/** The pinned window title doubles as the IPC-free badge channel. */
		expect(main).toContain('editor_window.on("page-title-updated", (event, title) => {');
		expect(main).toContain("event.preventDefault()");
		expect(main).toContain("AttentionCountFromTitle(title)");
		expect(main).toContain("AttentionOverlayImage(attention_count)");
		expect(main).toContain("AttentionOverlayDescription(attention_count)");
		expect(main).toContain('editor_window.setOverlayIcon(null, "")');
		expect(main).toContain("app.setBadgeCount(attention_count)");
		/** A waiting question outranks the count on the one-glyph overlay. */
		expect(main).toContain("TitleSignalsAwaitingAnswer(title)");
		expect(main).toContain(
			"editor_window.setOverlayIcon(QuestionOverlayImage(), question_overlay_description)",
		);
		/**
		 * The same channel carries the renderer's ask for a live Forge. The
		 * endpoint reaches a document once, in its launch fragment, so a Forge
		 * that respawned on another port is unreachable to it forever — only the
		 * shell can pair it again, and without this a crashed Forge left a
		 * disconnected window that only a restart could clear.
		 */
		expect(main).toContain("TitleRequestsForgeRepair(title)");
		expect(main).toContain("RequestForgeRepair()");
		expect(main).toContain("desktop_lifecycle.Reconnect()");
		/** Asked on every title rewrite, so only the transition may start work. */
		expect(main).toContain("repairing_forge = false");
		expect(main).toContain("repairing_forge = true");
		expect(main).not.toContain("preload:");
		expect(main).not.toContain("ipcMain");
		/** Freeze diagnosis remains opt-in and does not add a renderer bridge. */
		expect(main).toContain("ARTISAN_EDITOR_REMOTE_DEBUGGING_PORT");
		expect(main).toContain(
			'app.commandLine.appendSwitch("remote-debugging-address", "127.0.0.1")',
		);
		expect(main).toContain("ARTISAN_EDITOR_RENDERER_DIAGNOSTICS");
		expect(main).toContain("ARTISAN_DIAGNOSTICS_DIR");
		expect(main).toContain("CreateRendererDiagnosticLog");
		expect(main).toContain("ARTISAN_EDITOR_TRACE_FREEZES");
		expect(main).toContain("contentTracing.startRecording");
		expect(main).toContain("contentTracing.stopRecording");
		expect(main).toContain('recording_mode: "record-continuously"');
		expect(main).toContain("trace_buffer_size_in_kb");
		expect(main).toContain("devtools.timeline");
		expect(main).toContain("disabled-by-default-devtools.timeline.invalidationTracking");
		expect(main).toContain("disabled-by-default-devtools.timeline.frame");
		expect(main).toContain("disabled-by-default-devtools.timeline.stack");
		expect(main).toContain("disabled-by-default-v8.cpu_profiler");
		expect(main).toContain("disabled-by-default-v8.cpu_profiler.hires");
		expect(main).toContain("contentTracing.getTraceBufferUsage()");
		expect(main).toContain("trace-captured-late");
		expect(main).toContain("trace-capture-coalesced");
		expect(main).toContain("trace_capture_cooldown_ms");
		expect(main).toContain("trace-capture-timeout");
		expect(main).toContain("trace_capture_sequence");
		/** The heartbeat deliberately uses normal renderer evaluation, not a debugger attachment. */
		expect(main).toContain('executeJavaScript("void 0")');
		expect(main).toContain("heartbeat-stalled");
		expect(main).toContain("heartbeat-recovered");
		expect(main).toContain('input.key !== "F9"');
		expect(main).toContain("trace-capture-requested");
		expect(main).not.toContain("debugger.attach(");
		expect(main).toContain('editor_window.webContents.on("unresponsive"');
		expect(main).toContain('editor_window.webContents.on("responsive"');
		expect(main).toContain('editor_window.webContents.on("render-process-gone"');
		expect(main).toContain("app.getAppMetrics()");
		expect(main).toContain("process.getProcessMemoryInfo()");
		expect(main).toContain("process-metrics");
		expect(main).toContain("ARTISAN_EDITOR_OPEN_DEVTOOLS");
		expect(main).toContain('openDevTools({ mode: "detach" })');
		/** Renderer drafts persist, while every pairing begins without a Forge cookie. */
		expect(main).toContain('const renderer_partition = "persist:artisan-renderer"');
		expect(main).toContain("partition: renderer_partition");
		expect(main).toContain('clearStorageData({ storages: ["cookies"] })');
		expect(main.match(/clearStorageData\(/g)).toHaveLength(1);
		expect(main).not.toContain("clearStorageData()");
		expect(main).toContain("desktop_lifecycle.Start()");
		expect(main).toContain("desktop_lifecycle.Reconnect()");
		expect(main).toContain('app.on("before-quit"');
		expect(main).toContain('const windows_app_user_model_id = "com.usebarekey.artisan-editor"');
		expect(main).toContain(
			'const windows_toast_activator_clsid = "{A7D8D3E7-9DE2-4C09-8D4B-4E490C20D3A4}"',
		);
		expect(main).toContain("app.setAppUserModelId(windows_app_user_model_id)");
		expect(main).toContain("app.setToastActivatorCLSID(windows_toast_activator_clsid)");
		expect(main.indexOf("app.setAppUserModelId(windows_app_user_model_id)")).toBeLessThan(
			main.indexOf("app.whenReady"),
		);
		expect(
			main.indexOf("app.setToastActivatorCLSID(windows_toast_activator_clsid)"),
		).toBeLessThan(main.indexOf("app.whenReady"));
		expect(main).not.toContain("readShortcutLink");
		expect(main).not.toContain("writeShortcutLink");
		/** Pairing stays `ae`-owned: the editor only runs the hidden one-time handoff. */
		expect(main).toContain("make_node_forge_handoff_process_layer");
		expect(main).not.toContain("ARTISAN_AUTH_TOKEN");
		expect(main).not.toContain("ARTISAN_DATABASE_PATH");
	});
});

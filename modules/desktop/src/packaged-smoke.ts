import { randomUUID } from "node:crypto";

import { Effect, Layer, ManagedRuntime } from "effect";

import {
	ArtisanClient,
	adapt_electron_message_port_main,
	make_artisan_client_layer,
	MessagePortConnector,
	MessagePortConnectorError,
	TransportRuntimeLive,
} from "@artisan/transport";

import type { DesktopSmokeConnection, DesktopSmokeRenderer } from "./contracts";
import type { DesktopSessionSupervisor } from "./session-supervisor";

interface MainPortShape {
	readonly close: () => void;
	readonly off: (event: string, listener: (event?: unknown) => void) => unknown;
	readonly on: (event: string, listener: (event?: unknown) => void) => unknown;
	readonly postMessage: (message: unknown, transfer?: ReadonlyArray<object>) => void;
	readonly start: () => void;
}

/** Runs only when the packaged executable explicitly selects ARTISAN_PACKAGED_SMOKE=1. */
export const RunPackagedDesktopSmoke = async ({
	renderer,
	supervisor,
}: {
	readonly renderer: DesktopSmokeRenderer;
	readonly supervisor: DesktopSessionSupervisor;
}) => {
	const smoke_id = randomUUID();
	const thread_id = `packaged-desktop-smoke-thread-${smoke_id}`;
	const command_id = `packaged-desktop-smoke-create-${smoke_id}`;
	const ExecuteRenderer = <Result>(code: string) =>
		renderer.webContents.executeJavaScript(code, true) as Promise<Result>;
	const WaitForRenderer = async <Result>(code: string, description: string) => {
		const deadline = Date.now() + 15_000;
		while (Date.now() < deadline) {
			const result = await ExecuteRenderer<Result | undefined>(code);
			if (result !== undefined) return result;
			await new Promise<void>((resolve) => setTimeout(resolve, 50));
		}
		const diagnostic = await ExecuteRenderer<string>(
			`(() => JSON.stringify({ body: document.body?.innerText?.slice(0, 300) ?? "", keyboard_events: window.__artisanKeyboardSmokeEvents, native_click: window.__artisanNativeClick, native_click_events: window.__artisanNativeClickEvents, ready_state: document.readyState, thread_buttons: document.querySelectorAll(".thread-list button").length, url: location.href }))()`,
		);
		throw new Error(`Timed out waiting for packaged renderer ${description}: ${diagnostic}`);
	};
	const FocusNativeRenderer = async () => {
		const deadline = Date.now() + 5_000;
		do {
			renderer.show();
			renderer.restore();
			renderer.moveTop();
			renderer.focus();
			renderer.webContents.focus();
			if (renderer.isFocused()) {
				// Give Windows one turn to finish transferring foreground input
				// ownership before Chromium receives the synthesized native event.
				await new Promise<void>((resolve) => setTimeout(resolve, 50));
				if (renderer.isFocused()) return;
			}
			await new Promise<void>((resolve) => setTimeout(resolve, 50));
		} while (Date.now() < deadline);
		throw new Error("Packaged renderer BrowserWindow did not acquire native focus");
	};
	const InspectRenderer = async () =>
		ExecuteRenderer<{
			readonly accessible_names: ReadonlyArray<string>;
			readonly bridge_available: boolean;
			readonly focused_name: string | undefined;
			readonly grid_template_columns: string;
			readonly left_visible: boolean;
			readonly right_visible: boolean;
		}>(`(() => {
			const shell = document.querySelector(".app-shell");
			const names = Array.from(document.querySelectorAll("button[aria-label]"))
				.map((button) => button.getAttribute("aria-label") ?? "")
				.filter(Boolean);
			const visible = (selector) => {
				const element = document.querySelector(selector);
				return element !== null && getComputedStyle(element).display !== "none";
			};
			return {
				accessible_names: names,
				bridge_available:
					typeof window.artisanDesktop?.requestConnection === "function" &&
					typeof window.artisanDesktop?.setWorking === "function",
				focused_name: document.activeElement?.getAttribute("aria-label") ?? undefined,
				grid_template_columns: shell === null ? "" : getComputedStyle(shell).gridTemplateColumns,
				left_visible: visible(".app-sidebar"),
				right_visible: visible(".app-content"),
			};
		})()`);
	const FocusAndKeyboardCreateThread = async () => {
		// Chromium only marks the synthesized click as trusted when its native
		// BrowserWindow is the foreground input target. Do not replace this with a
		// DOM click: this release gate specifically proves Electron keyboard input.
		await FocusNativeRenderer();
		const count_before = await ExecuteRenderer<number>(
			`(() => document.querySelectorAll(".thread-links a").length)()`,
		);
		const focused = await ExecuteRenderer<boolean>(
			`(() => {
				const button = document.querySelector('button[aria-label="New chat"]');
				button?.focus();
				window.__artisanKeyboardSmokeEvents = [];
				window.addEventListener("keydown", (event) => {
					window.__artisanKeyboardSmokeEvents.push({
						code: event.code,
						is_trusted: event.isTrusted,
						key: event.key,
						kind: "keydown",
					});
				}, { once: true, capture: true });
				window.addEventListener("keyup", (event) => {
					window.__artisanKeyboardSmokeEvents.push({
						code: event.code,
						is_trusted: event.isTrusted,
						key: event.key,
						kind: "keyup",
					});
				}, { once: true, capture: true });
				button?.addEventListener("click", (event) => {
					window.__artisanKeyboardSmokeEvents.push({
						active_before_click: document.activeElement === button,
						detail: event.detail,
						is_trusted: event.isTrusted,
						kind: "click",
					});
				}, { once: true });
				return document.hasFocus() && document.activeElement === button;
			})()`,
		);
		if (!focused) throw new Error("Packaged renderer could not focus the New chat control");
		if (!renderer.isFocused())
			throw new Error("Packaged renderer lost native focus before keyboard input");
		renderer.webContents.sendInputEvent({ keyCode: "Space", type: "rawKeyDown" });
		renderer.webContents.sendInputEvent({ keyCode: "Space", type: "char" });
		renderer.webContents.sendInputEvent({ keyCode: "Space", type: "keyUp" });
		const keyboard_activation = await WaitForRenderer<{
			readonly event: {
				readonly active_before_click: boolean;
				readonly detail: number;
				readonly is_trusted: boolean;
			};
			readonly thread_count: number;
		}>(
			`(() => {
				const thread_count = document.querySelectorAll(".thread-links a").length;
				const event = window.__artisanKeyboardSmokeEvents?.find((candidate) => candidate.kind === "click");
				return location.pathname.startsWith("/thread/") && thread_count > ${count_before} && event?.is_trusted && event.detail === 0 && event.active_before_click
					? { event, thread_count }
					: undefined;
			})()`,
			"keyboard-originated New chat command",
		);
		return { focused_name: "New chat", ...keyboard_activation };
	};
	const NativeClickControl = async (accessible_name: string) => {
		await FocusNativeRenderer();
		const target = await ExecuteRenderer<
			| { readonly target_name: string | null; readonly x: number; readonly y: number }
			| undefined
		>(`(() => {
			const button = document.querySelector(${JSON.stringify(`[aria-label="${accessible_name}"]`)});
			if (!(button instanceof HTMLElement) || button.matches(":disabled")) return undefined;
			const bounds = button.getBoundingClientRect();
			if (bounds.width <= 0 || bounds.height <= 0) return undefined;
			window.__artisanNativeClick = undefined;
			window.__artisanNativeClickEvents = [];
			for (const kind of ["pointermove", "mousemove", "pointerdown", "mousedown", "pointerup", "mouseup", "click"]) {
				button.addEventListener(kind, (event) => {
					window.__artisanNativeClickEvents.push({ detail: event.detail, is_trusted: event.isTrusted, kind });
					if (kind === "click") {
						window.__artisanNativeClick = { detail: event.detail, is_trusted: event.isTrusted };
					}
				}, { once: true });
			}
			const x = Math.round(bounds.left + bounds.width / 2);
			const y = Math.round(bounds.top + bounds.height / 2);
			return { target_name: document.elementFromPoint(x, y)?.getAttribute("aria-label") ?? null, x, y };
		})()`);
		if (target === undefined) {
			throw new Error(`Packaged renderer could not target ${accessible_name}`);
		}
		const position = { x: target.x, y: target.y };
		renderer.webContents.sendInputEvent({ ...position, type: "mouseMove" });
		renderer.webContents.sendInputEvent({
			...position,
			button: "left",
			clickCount: 1,
			type: "mouseDown",
		});
		renderer.webContents.sendInputEvent({
			...position,
			button: "left",
			clickCount: 1,
			type: "mouseUp",
		});
		return WaitForRenderer<{ readonly detail: number; readonly is_trusted: boolean }>(
			`(() => {
				const event = window.__artisanNativeClick;
				return event?.is_trusted && event.detail === 1 ? event : undefined;
			})()`,
			`trusted click for ${accessible_name}`,
		);
	};
	const InspectMountedInteractions = async () => {
		const modes: Array<"Chat" | "Editor" | "Orchestrator"> = [
			"Chat",
			"Orchestrator",
			"Editor",
			"Chat",
		];
		const mode_evidence = [];
		for (const mode of modes) {
			const click = await NativeClickControl(mode);
			const visible_region = await WaitForRenderer<string>(
				`(() => document.querySelector('[aria-label="${mode}"]') !== null && document.querySelector('button[aria-label="${mode}"]')?.getAttribute("aria-pressed") === "true" ? "${mode}" : undefined)()`,
				`${mode} mode visibility`,
			);
			mode_evidence.push({ click, visible_region });
		}
		const composer_focused = await ExecuteRenderer<boolean>(
			`(() => {
				const composer = document.querySelector('textarea[aria-label="Message Codex"]');
				if (!(composer instanceof HTMLTextAreaElement)) return false;
				window.__artisanComposerInput = undefined;
				composer.addEventListener("input", (event) => {
					window.__artisanComposerInput = { is_trusted: event.isTrusted, value: composer.value };
				}, { once: true });
				composer.focus();
				return document.hasFocus() && document.activeElement === composer;
			})()`,
		);
		if (!composer_focused)
			throw new Error("Packaged renderer could not focus the chat composer");
		renderer.webContents.sendInputEvent({ keyCode: "H", type: "keyDown" });
		renderer.webContents.sendInputEvent({ keyCode: "h", type: "char" });
		renderer.webContents.sendInputEvent({ keyCode: "H", type: "keyUp" });
		const composer = await WaitForRenderer<{
			readonly is_trusted: boolean;
			readonly value: string;
		}>(
			`(() => {
				const input = window.__artisanComposerInput;
				return input?.is_trusted && input.value.includes("h") ? input : undefined;
			})()`,
			"trusted chat composer input",
		);
		const editor_restore_click = await NativeClickControl("Editor");
		const editor_restored = await WaitForRenderer<boolean>(
			`(() => document.querySelector('button[aria-label="Editor"]')?.getAttribute("aria-pressed") === "true" ? true : undefined)()`,
			"Editor mode restoration",
		);
		const activity_bridge = await ExecuteRenderer<boolean>(`(async () => {
			await window.artisanDesktop.setWorking(true);
			await window.artisanDesktop.setWorking(false);
			return true;
		})()`);
		const unavailable = await ExecuteRenderer<{
			readonly no_active_file: boolean;
			readonly no_terminal_sessions: boolean;
		}>(`(() => ({
			no_active_file: document.body.textContent?.includes("Open a workspace file") ?? false,
			no_terminal_sessions: document.body.textContent?.includes("No terminal sessions.") ?? false,
		}))()`);
		const route_path = await ExecuteRenderer<string>("location.pathname");
		return {
			activity_bridge,
			composer,
			editor_restore: { click: editor_restore_click, restored: editor_restored },
			modes: mode_evidence,
			route_path,
			unavailable,
		};
	};
	const connector = Layer.succeed(MessagePortConnector, {
		Connect: Effect.acquireRelease(
			Effect.gen(function* () {
				const connection: DesktopSmokeConnection = supervisor.RequestSmokeConnection();
				return {
					control_port: yield* adapt_electron_message_port_main(
						connection.control_port as MainPortShape,
					),
					stream_port: yield* adapt_electron_message_port_main(
						connection.stream_port as MainPortShape,
					),
				};
			}),
			({ control_port, stream_port }) => Effect.all([control_port.Close, stream_port.Close]),
		).pipe(Effect.mapError((cause) => new MessagePortConnectorError({ cause }))),
	});
	const runtime = ManagedRuntime.make(
		make_artisan_client_layer({ reconnect_delay_ms: 25 }).pipe(
			Layer.provide(connector),
			Layer.provide(TransportRuntimeLive),
		),
	);
	try {
		return await runtime.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					renderer.setBounds({ height: 900, width: 1440 });
					const wide = yield* Effect.promise(() =>
						WaitForRenderer(
							`(() => document.querySelector(".app-shell") === null ? undefined : true)()`,
							"mount",
						),
					);
					if (!wide)
						return yield* Effect.fail(new Error("Packaged renderer did not mount"));
					const wide_ui = yield* Effect.promise(InspectRenderer);
					const first_renderer_thread_count = yield* Effect.promise(
						FocusAndKeyboardCreateThread,
					);
					const product_interactions = yield* Effect.promise(InspectMountedInteractions);
					const initial_native_load = yield* Effect.promise(() =>
						supervisor.AwaitSmokeNativeLoad(1),
					).pipe(Effect.timeout("15 seconds"));
					const client = yield* ArtisanClient;
					const first = yield* client.Command({
						command_id,
						payload: { title: "Packaged desktop smoke", type: "thread.create" },
						thread_id,
					});
					yield* client.ListThreads;
					const restart = yield* Effect.promise(() =>
						supervisor.ForceRestartForSmoke(),
					).pipe(Effect.timeout("15 seconds"));
					const restarted_native_load = yield* Effect.promise(() =>
						supervisor.AwaitSmokeNativeLoad(restart.next_utility_epoch),
					).pipe(Effect.timeout("15 seconds"));
					const restarted_renderer_thread_count = yield* Effect.promise(
						FocusAndKeyboardCreateThread,
					);
					renderer.setBounds({ height: 900, width: 700 });
					yield* Effect.promise(() =>
						ExecuteRenderer(
							"new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)))",
						),
					);
					const narrow_ui = yield* Effect.promise(InspectRenderer);
					renderer.webContents.setZoomFactor(2);
					const zoom_factor = renderer.webContents.getZoomFactor();
					const replay = yield* client.Command({
						command_id,
						payload: { title: "Packaged desktop smoke", type: "thread.create" },
						thread_id,
					});
					const forward = yield* client.Command({
						command_id: `packaged-desktop-smoke-forward-${smoke_id}`,
						payload: { title: "Packaged desktop forward smoke", type: "thread.create" },
						thread_id: `packaged-desktop-smoke-forward-thread-${smoke_id}`,
					});
					const threads = yield* client.ListThreads;
					if (
						first.status !== "accepted" ||
						replay.status !== "duplicate" ||
						forward.status !== "accepted" ||
						threads.filter((thread) => thread.thread_id === thread_id).length !== 1 ||
						restart.next_utility_epoch <= restart.previous_utility_epoch ||
						restarted_native_load.utility_epoch !== restart.next_utility_epoch ||
						forward.journal_sequence <= first.journal_sequence ||
						!wide_ui.bridge_available ||
						!wide_ui.accessible_names.includes("New chat") ||
						wide_ui.grid_template_columns.split(" ").length < 2 ||
						!wide_ui.left_visible ||
						!wide_ui.right_visible ||
						!product_interactions.activity_bridge ||
						!product_interactions.route_path.startsWith("/thread/") ||
						narrow_ui.grid_template_columns.split(" ").length !== 1 ||
						narrow_ui.left_visible ||
						!narrow_ui.right_visible ||
						zoom_factor !== 2
					)
						return yield* Effect.fail(
							new Error(
								`Packaged utility restart did not preserve exact replay: ${JSON.stringify({ first, forward, narrow_ui, product_interactions, replay, restarted_native_load, restart, threads, wide_ui, zoom_factor })}`,
							),
						);
					return {
						forward_generation: true,
						mounted_ui: {
							keyboard_thread_counts: {
								initial: first_renderer_thread_count,
								restarted: restarted_renderer_thread_count,
							},
							product_interactions,
							narrow: narrow_ui,
							wide: wide_ui,
							zoom_factor,
						},
						native_load: {
							initial: initial_native_load,
							restarted: restarted_native_load,
						},
						restart,
						thread_count: 1,
					};
				}),
			).pipe(Effect.timeout("45 seconds")),
		);
	} finally {
		await runtime.dispose();
	}
};

import { contextBridge, ipcRenderer } from "electron";

import { DesktopSessionConnectionType } from "@artisan/transport/desktop-session";

const request_channel = "artisan:request-connection";
const connection_channel = "artisan:connection";
const renderer_window = globalThis as typeof globalThis & {
	readonly location: { readonly origin: string };
	readonly postMessage: (
		message: unknown,
		target_origin: string,
		transfer: ReadonlyArray<object>,
	) => void;
};

function connection_message(
	value: unknown,
): { readonly generation: number; readonly kind: string } | undefined {
	if (typeof value !== "object" || value === null) {
		return undefined;
	}

	const candidate = value as { readonly generation?: unknown; readonly kind?: unknown };

	return typeof candidate.kind === "string" && typeof candidate.generation === "number"
		? { generation: candidate.generation, kind: candidate.kind }
		: undefined;
}

/** Forwards transferred ports into main world without exposing Electron's IPC surface. */
ipcRenderer.on(connection_channel, (event, message: unknown) => {
	const connection = connection_message(message);

	if (
		connection === undefined ||
		connection.kind !== "artisan:connection" ||
		!Number.isSafeInteger(connection.generation) ||
		connection.generation < 1 ||
		event.ports.length !== 2
	) {
		return;
	}

	renderer_window.postMessage(
		{ generation: connection.generation, type: DesktopSessionConnectionType },
		renderer_window.location.origin,
		event.ports,
	);
});

contextBridge.exposeInMainWorld("artisanDesktop", {
	requestConnection: () => ipcRenderer.invoke(request_channel).then(() => undefined),
});

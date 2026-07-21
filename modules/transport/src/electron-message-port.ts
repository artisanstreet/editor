import { make_message_port_like, type MessagePortAdapterOptions } from "./message-port";

/** Describes only the Electron MessagePortMain members used by the adapter. */
export interface ElectronMessagePortMainShape {
	readonly close: () => void;
	readonly off: (event: string, listener: (event?: unknown) => void) => unknown;
	readonly on: (event: string, listener: (event?: unknown) => void) => unknown;
	readonly postMessage: (message: unknown, transfer?: ReadonlyArray<object>) => void;
	readonly start: () => void;
}

/** Describes only the renderer MessagePort members used by the adapter. */
export interface ElectronRendererMessagePortShape {
	readonly addEventListener: (event: string, listener: (event: unknown) => void) => void;
	readonly close: () => void;
	readonly postMessage: (message: unknown, transfer?: ReadonlyArray<object>) => void;
	readonly removeEventListener: (event: string, listener: (event: unknown) => void) => void;
	readonly start: () => void;
}

function event_data(event: unknown) {
	return typeof event === "object" && event !== null && "data" in event ? event.data : undefined;
}

/** Adapts an Electron MessagePortMain-compatible emitter without importing Electron. */
export function adapt_electron_message_port_main(
	port: ElectronMessagePortMainShape,
	options: MessagePortAdapterOptions = {},
) {
	return make_message_port_like(
		{
			add_close_listener: (listener) => {
				port.on("close", listener);

				return () => {
					port.off("close", listener);
				};
			},
			add_message_error_listener: (listener) => {
				const wrapped = (event?: unknown) => listener(event);

				port.on("messageerror", wrapped);

				return () => {
					port.off("messageerror", wrapped);
				};
			},
			add_message_listener: (listener) => {
				const wrapped = (event?: unknown) => listener(event_data(event));

				port.on("message", wrapped);

				return () => {
					port.off("message", wrapped);
				};
			},
			close: () => port.close(),
			post_message: (message, transfer) => {
				if (transfer) {
					port.postMessage(message, transfer);

					return;
				}

				port.postMessage(message);
			},
			start: () => port.start(),
		},
		options,
	);
}

/** Adapts an Electron renderer MessagePort-compatible EventTarget without Electron. */
export function adapt_electron_renderer_message_port(
	port: ElectronRendererMessagePortShape,
	options: MessagePortAdapterOptions = {},
) {
	return make_message_port_like(
		{
			add_close_listener: (listener) => {
				port.addEventListener("close", listener);

				return () => port.removeEventListener("close", listener);
			},
			add_message_error_listener: (listener) => {
				const wrapped = (event: unknown) => listener(event);

				port.addEventListener("messageerror", wrapped);

				return () => port.removeEventListener("messageerror", wrapped);
			},
			add_message_listener: (listener) => {
				const wrapped = (event: unknown) => listener(event_data(event));

				port.addEventListener("message", wrapped);

				return () => port.removeEventListener("message", wrapped);
			},
			close: () => port.close(),
			post_message: (message, transfer) => {
				if (transfer) {
					port.postMessage(message, transfer);

					return;
				}

				port.postMessage(message);
			},
			start: () => port.start(),
		},
		options,
	);
}

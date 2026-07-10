import type { MessagePort, Transferable } from "node:worker_threads";

import { make_message_port_like, type MessagePortAdapterOptions } from "./message-port";

/** Adapts a Node worker_threads MessagePort into the shell-neutral interface. */
export function adapt_node_message_port(
	port: MessagePort,
	options: MessagePortAdapterOptions = {},
) {
	return make_message_port_like(
		{
			add_close_listener: (listener) => {
				port.on("close", listener);

				return () => port.off("close", listener);
			},
			add_message_error_listener: (listener) => {
				port.on("messageerror", listener);

				return () => port.off("messageerror", listener);
			},
			add_message_listener: (listener) => {
				port.on("message", listener);

				return () => port.off("message", listener);
			},
			close: () => port.close(),
			post_message: (message, transfer) => {
				if (transfer) {
					port.postMessage(message, Array.from(transfer) as Array<Transferable>);

					return;
				}

				port.postMessage(message);
			},
			start: () => port.start(),
		},
		options,
	);
}

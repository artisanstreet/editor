import { Effect, Layer } from "effect";

import { BackendBinaryStreamSourceLive } from "./stream-source";
import {
	MessagePortTransportServer,
	type MessagePortTransportServerOptions,
} from "./server-contract";
import { TransportRuntimeLive } from "./transport-runtime";
import { make_server_binding } from "./internal/server-binding";
import { server_error, validate_server_options } from "./internal/server-common";

/** Re-exports the public MessagePort transport server contract. */
export * from "./server-contract";

/** Re-exports the injectable terminal and asset stream source contract. */
export * from "./stream-source";

/** Builds a scoped MessagePort server binder over the existing ProtocolServer. */
export function make_message_port_transport_server_layer(
	input_options: MessagePortTransportServerOptions = {},
) {
	const options: Required<MessagePortTransportServerOptions> = {
		max_active_streams: input_options.max_active_streams ?? 16,
		stream_outbound_capacity: input_options.stream_outbound_capacity ?? 64,
	};

	return Layer.effect(
		MessagePortTransportServer,
		validate_server_options(options)
			? make_server_binding(options)
			: Effect.fail(
					server_error(
						"configuration",
						new Error("transport server limits must be positive safe integers"),
					),
				),
	);
}

/** Provides the production runtime and backend terminal/asset stream source. */
export function make_backend_message_port_transport_server_layer(
	options: MessagePortTransportServerOptions = {},
) {
	return make_message_port_transport_server_layer(options).pipe(
		Layer.provide(TransportRuntimeLive),
		Layer.provide(BackendBinaryStreamSourceLive),
	);
}

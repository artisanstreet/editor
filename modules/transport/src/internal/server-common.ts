import type { MessagePortError } from "../message-port";
import {
	MessagePortTransportServerError,
	type MessagePortTransportServerErrorCode,
	type MessagePortTransportServerOptions,
} from "../server-contract";

/** Creates one consistently shaped server transport failure. */
export function server_error(code: MessagePortTransportServerErrorCode, cause: unknown) {
	return new MessagePortTransportServerError({ cause, code });
}

/** Converts normalized port errors at the server transport boundary. */
export function map_server_port_error(cause: MessagePortError) {
	return server_error("port", cause);
}

/** Validates server stream limits before opening protocol or native resources. */
export function validate_server_options(options: Required<MessagePortTransportServerOptions>) {
	return (
		Number.isSafeInteger(options.max_active_streams) &&
		options.max_active_streams > 0 &&
		Number.isSafeInteger(options.stream_outbound_capacity) &&
		options.stream_outbound_capacity > 0
	);
}

/** Supported renderer entry; its import graph excludes backend and Node modules. */
export * from "./client-contract";

/** Exposes the scoped client layer constructor. */
export { make_artisan_client_layer } from "./internal/client-service";

/** Renderer-safe connection primitives for the desktop preload boundary. */
export { MessagePortConnector, MessagePortConnectorError } from "./connector";
export {
	adapt_electron_renderer_message_port,
	type ElectronRendererMessagePortShape,
} from "./electron-message-port";
export {
	DesktopSessionConnectionType,
	type DesktopIdentity,
	type DesktopSessionBridge,
	type DesktopSessionConnection,
} from "./desktop-session";
export { TransportRuntime, TransportRuntimeLive } from "./transport-runtime";

/** Supported renderer entry; its import graph excludes backend and Node modules. */
export * from "./client-api/contract";
export * from "./client-api/service";

/** Exposes the scoped client layer constructor. */
export { make_artisan_client_layer } from "./internal/client-service";

/** Renderer-safe connection primitives used by the WebSocket adapter. */
export { MessagePortConnector, MessagePortConnectorError } from "./connector";
export { TransportRuntime, TransportRuntimeLive } from "./transport-runtime";

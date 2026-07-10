/** Supported renderer entry; its import graph excludes backend and Node modules. */
export * from "./client-contract";

/** Exposes the scoped client layer constructor. */
export { make_artisan_client_layer } from "./internal/client-service";

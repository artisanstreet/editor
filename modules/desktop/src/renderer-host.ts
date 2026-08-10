import { readFile } from "node:fs/promises";
import { extname, join, normalize, sep } from "node:path";

import { Data, Effect, Option, Schema } from "effect";

export class DesktopLauncherError extends Data.TaggedError("DesktopLauncherError")<{
	readonly cause?: unknown;
	readonly reason: "handoff_failed" | "handoff_invalid";
}> {}

/**
 * The renderer is served from a privileged standard scheme so `artisan://app`
 * is a real origin: SvelteKit history routing, module scripts, and fetch all
 * behave exactly as on an HTTP origin.
 */
export const app_scheme = "artisan";
export const app_host = "app";

const LoopbackForgeEndpoint = Schema.String.check(
	Schema.isMaxLength(256),
	Schema.makeFilter<string>((input) => {
		try {
			const url = new URL(input);
			return url.protocol === "http:" &&
				(url.hostname === "127.0.0.1" ||
					url.hostname === "::1" ||
					url.hostname === "[::1]") &&
				url.username === "" &&
				url.password === "" &&
				url.port !== ""
				? undefined
				: "Expected a loopback HTTP Forge endpoint with an explicit port";
		} catch {
			return "Expected a loopback HTTP Forge endpoint";
		}
	}),
);

/**
 * The one-time pairing capability handed to this trusted local caller by
 * `ae open --handoff`. It is single-use and short-lived; the editor never
 * reads Forge secrets or any durable credential.
 */
export const ForgeHandoff = Schema.Struct({
	endpoint: LoopbackForgeEndpoint,
	owned_instance_id: Schema.optional(
		Schema.String.check(Schema.isPattern(/^[A-Za-z0-9_-]{1,128}$/), Schema.isMaxLength(128)),
	),
	pair_code: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(256)),
	version: Schema.Literal(1),
});
export type ForgeHandoff = typeof ForgeHandoff.Type;

export const DecodeHandoffOutput = (stdout: string) =>
	Effect.gen(function* () {
		for (const line of stdout.split(/\r?\n/)) {
			const candidate = line.trim();
			if (!candidate.startsWith("{")) continue;
			const parsed = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
				candidate,
			).pipe(Effect.option);
			if (Option.isNone(parsed)) continue;
			const decoded = yield* Schema.decodeUnknownEffect(ForgeHandoff)(parsed.value).pipe(
				Effect.option,
			);
			if (Option.isSome(decoded)) return decoded.value;
		}
		return yield* Effect.fail(new DesktopLauncherError({ reason: "handoff_invalid" }));
	});

const mime_types: Readonly<Record<string, string>> = {
	".css": "text/css; charset=utf-8",
	".html": "text/html; charset=utf-8",
	".ico": "image/x-icon",
	".js": "text/javascript; charset=utf-8",
	".json": "application/json; charset=utf-8",
	".png": "image/png",
	".svg": "image/svg+xml",
	".txt": "text/plain; charset=utf-8",
	".webmanifest": "application/manifest+json",
	".woff2": "font/woff2",
};

/**
 * Serves the bundled static frontend for `artisan://app`. Extensionless paths
 * fall back to `index.html` so client-side routes survive reloads, exactly
 * like the development Forge's SPA fallback. Everything else is a plain file
 * read confined to the packaged frontend directory.
 */
export const ServeRendererAsset = (frontend_root: string, request_url: string) =>
	Effect.gen(function* () {
		const url = yield* Effect.try(() => new URL(request_url)).pipe(Effect.option);
		if (Option.isNone(url) || url.value.host !== app_host) {
			return new Response(null, { status: 404 });
		}
		const decoded_pathname = yield* Effect.try(() =>
			decodeURIComponent(url.value.pathname),
		).pipe(Effect.option);
		if (Option.isNone(decoded_pathname)) return new Response(null, { status: 400 });
		const pathname = decoded_pathname.value;
		const relative_path =
			pathname === "/" || extname(pathname) === ""
				? "index.html"
				: pathname.replace(/^\/+/, "");
		const resolved = normalize(join(frontend_root, relative_path));
		if (resolved !== frontend_root && !resolved.startsWith(`${frontend_root}${sep}`)) {
			return new Response(null, { status: 403 });
		}
		const bytes = yield* Effect.tryPromise(() => readFile(resolved)).pipe(Effect.option);
		/** Missing assets 404; missing extensionless routes already used the fallback. */
		if (Option.isNone(bytes)) return new Response(null, { status: 404 });
		return new Response(new Uint8Array(bytes.value), {
			headers: {
				"content-type": mime_types[extname(resolved)] ?? "application/octet-stream",
			},
			status: 200,
		});
	});

/**
 * The launch URL carries the one-time pairing capability and the home's
 * loopback Forge endpoint in the fragment, which never reaches any network
 * request; the renderer consumes and strips it exactly once.
 */
export const renderer_url = (handoff: Option.Option<ForgeHandoff>) =>
	Option.isSome(handoff)
		? `${app_scheme}://${app_host}/#pair=${encodeURIComponent(handoff.value.pair_code)}&forge=${encodeURIComponent(handoff.value.endpoint)}`
		: `${app_scheme}://${app_host}/`;

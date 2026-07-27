import { appendFileSync, readFileSync } from "node:fs";
import { basename, join } from "node:path";

const release_root = process.env.ARTISAN_ACCEPTANCE_RELEASE_ROOT;
const request_log = process.env.ARTISAN_ACCEPTANCE_FETCH_LOG;

if (release_root === undefined || request_log === undefined)
	throw new Error("The local release preload requires an explicit release root and request log");

const allowed_assets = new Set([
	"release-manifest.json",
	"release-manifest.sig",
	...JSON.parse(readFileSync(join(release_root, "release-manifest.json"), "utf8")).artifacts.map(
		(artifact: { readonly file_name: string }) => artifact.file_name,
	),
]);

const native_fetch = globalThis.fetch;
globalThis.fetch = async (input, init) => {
	const url = new URL(input instanceof Request ? input.url : String(input));
	if (
		(url.protocol === "http:" || url.protocol === "https:") &&
		(url.hostname === "127.0.0.1" || url.hostname === "::1" || url.hostname === "localhost")
	) {
		const response = await native_fetch(input, init);
		appendFileSync(request_log, `${url.href} ${response.status}\n`, "utf8");
		return response;
	}
	if (
		url.protocol !== "https:" ||
		url.hostname !== "github.com" ||
		!url.pathname.startsWith("/sandersonstabo/artisan-editor/releases/latest/download/")
	)
		throw new Error(`Acceptance preload rejected unexpected network access: ${url}`);
	const asset = basename(url.pathname);
	if (!allowed_assets.has(asset))
		throw new Error(`Acceptance preload rejected unknown release asset: ${asset}`);
	const bytes = readFileSync(join(release_root, asset));
	appendFileSync(request_log, `${url.href}\n`, "utf8");
	return new Response(bytes, {
		headers: { "content-length": String(bytes.byteLength) },
		status: 200,
	});
};

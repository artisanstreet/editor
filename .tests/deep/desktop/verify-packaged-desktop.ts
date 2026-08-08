import { existsSync, readdirSync } from "node:fs";
import { resolve } from "node:path";

import { extractFile, listPackage } from "@electron/asar";

const repository_root = resolve(import.meta.dirname, "../../..");
const artifact_release_root = resolve(repository_root, ".dist/electron-release/win-unpacked");
const artifact_asar = resolve(artifact_release_root, "resources/app.asar");
const artifact_executable = resolve(artifact_release_root, "Artisan Editor.exe");

for (const path of [artifact_asar, artifact_executable]) {
	if (!existsSync(path)) throw new Error(`Missing packaged desktop evidence path: ${path}`);
}

const asar_entries = listPackage(artifact_asar, { isPack: false }).map((entry) =>
	entry.replaceAll("\\", "/"),
);
const contains_entry = (path: string) =>
	asar_entries.includes(path) || asar_entries.some((entry) => entry.startsWith(`${path}/`));

for (const entry of ["/main.js", "/frontend/index.html", "/frontend/_app"]) {
	if (!contains_entry(entry)) throw new Error(`Missing ASAR entry: ${entry}`);
}
for (const entry of ["/preload.cjs", "/preload.js", "/utility.js"]) {
	if (asar_entries.includes(entry))
		throw new Error(`A privileged renderer bridge is packaged: ${entry}`);
}
for (const entry of ["/native-runtime", "/migrations", "/modules/backend/drizzle"]) {
	if (contains_entry(entry))
		throw new Error(`Forge-owned payload leaked into the Electron ASAR: ${entry}`);
}

const shell_document = extractFile(artifact_asar, "frontend/index.html").toString("utf8");
if (!shell_document.includes("connect-src 'self' http://127.0.0.1:* ws://127.0.0.1:*")) {
	throw new Error("The packaged renderer shell lacks the loopback Forge CSP allowance");
}
if (!shell_document.includes("object-src 'none'")) {
	throw new Error("The packaged renderer shell weakened its object-src policy");
}
if (/connect-src[^;]*https?:\/\/(?!127\.0\.0\.1|\[::1\])/u.test(shell_document)) {
	throw new Error("The packaged renderer shell allows a non-loopback connect origin");
}

const embedded_forge = resolve(artifact_release_root, "resources/artisan-forge");
if (existsSync(embedded_forge)) {
	throw new Error("The managed Editor payload must not embed a parallel Forge lifecycle");
}

const parallel_installers = readdirSync(resolve(artifact_release_root, ".."), {
	withFileTypes: true,
}).filter(
	(entry) => entry.isFile() && (entry.name.includes("Setup") || entry.name.endsWith(".blockmap")),
);
if (parallel_installers.length > 0) {
	throw new Error("The desktop package emitted a parallel installer lifecycle");
}

console.log(
	"Packaged desktop renderer evidence:",
	JSON.stringify({
		asar_entries: asar_entries.length,
		bundled_frontend: true,
		forge_payload_embedded: false,
		loopback_csp: true,
		managed_distribution_payload: true,
		ok: true,
		privileged_bridge: false,
	}),
);

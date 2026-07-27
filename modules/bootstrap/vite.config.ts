import { createPublicKey } from "node:crypto";
import { readFileSync } from "node:fs";
import { chmod } from "node:fs/promises";
import { resolve } from "node:path";

import { defineConfig } from "vite";

const ProductionTrust = () => {
	const key_id = process.env.ARTISAN_RELEASE_SIGNING_KEY_ID;
	const private_key_pem =
		process.env.ARTISAN_RELEASE_SIGNING_KEY_PEM ??
		(process.env.ARTISAN_RELEASE_SIGNING_KEY_FILE === undefined
			? undefined
			: readFileSync(process.env.ARTISAN_RELEASE_SIGNING_KEY_FILE, "utf8"));
	if (!key_id || !private_key_pem)
		throw new Error(
			"Production bootstrap requires ARTISAN_RELEASE_SIGNING_KEY_ID and a signing private key",
		);
	return {
		key_id,
		public_key_base64: createPublicKey(private_key_pem)
			.export({ format: "der", type: "spki" })
			.toString("base64"),
	};
};

export default defineConfig(({ mode }) => {
	const trust =
		mode === "artisan-production" ? ProductionTrust() : { key_id: "", public_key_base64: "" };
	return {
		define: {
			__ARTISAN_RELEASE_PUBLIC_KEY_BASE64__: JSON.stringify(trust.public_key_base64),
			__ARTISAN_RELEASE_SIGNING_KEY_ID__: JSON.stringify(trust.key_id),
		},
		plugins: [
			{
				name: "artisan-bootstrap-executable",
				closeBundle: () => chmod(resolve(import.meta.dirname, ".dist", "entry.js"), 0o755),
			},
		],
		ssr: {
			noExternal: true,
		},
		build: {
			emptyOutDir: true,
			minify: false,
			outDir: ".dist",
			rollupOptions: {
				input: resolve(import.meta.dirname, "src", "entry.ts"),
				output: {
					entryFileNames: "entry.js",
					format: "es",
				},
			},
			ssr: true,
			target: "node22",
		},
	};
});

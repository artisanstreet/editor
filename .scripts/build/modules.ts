import { resolve } from "node:path";

/**
 * The workspace packages that are bundled into their own artifact. Each one is
 * an importable TypeScript library; the modules left out have their own
 * pipelines (`frontend` and `installer` build through Vite) or ship no code at
 * all (`data` is a JSON asset tree).
 */

export const repository_root = resolve(import.meta.dirname, "..", "..");

export interface BundledModule {
	readonly directory: string;
	/** Export subpath to the source file behind it, mirroring package.json. */
	readonly entries: Readonly<Record<string, string>>;
	readonly name: string;
	/**
	 * Editor modules resolve to their bundle by default. The two terminal
	 * dashboards stay on source: their consumers are the plain Node scripts in
	 * `.scripts`, including the one that produces these bundles.
	 */
	readonly ships_in_editor: boolean;
}

export const bundled_modules: ReadonlyArray<BundledModule> = [
	{
		directory: "modules/backend",
		entries: { ".": "src/index.ts" },
		name: "@artisan/backend",
		ships_in_editor: true,
	},
	{
		directory: "modules/catalog",
		entries: { ".": "src/index.ts" },
		name: "@artisan/catalog",
		ships_in_editor: true,
	},
	{
		directory: "modules/cli",
		entries: { ".": "src/index.ts", "./entry": "src/entry.ts" },
		name: "@artisan/cli",
		ships_in_editor: true,
	},
	{
		directory: "modules/desktop",
		entries: {
			".": "src/index.ts",
			"./renderer-diagnostics": "src/renderer-diagnostics.ts",
		},
		name: "@artisan/desktop",
		ships_in_editor: true,
	},
	{
		directory: "modules/distribution",
		entries: { ".": "src/index.ts" },
		name: "@artisan/distribution",
		ships_in_editor: true,
	},
	{
		directory: "modules/engines",
		entries: { ".": "src/index.ts" },
		name: "@artisan/engines",
		ships_in_editor: true,
	},
	{
		directory: "modules/forge",
		entries: { ".": "src/index.ts", "./entry": "src/entry.ts" },
		name: "@artisan/forge",
		ships_in_editor: true,
	},
	{
		directory: "modules/protocol",
		entries: { ".": "src/index.ts" },
		name: "@artisan/protocol",
		ships_in_editor: true,
	},
	{
		directory: "modules/observability",
		entries: { ".": "src/index.ts" },
		name: "@artisan/observability",
		ships_in_editor: true,
	},
	{
		directory: "modules/transport",
		entries: {
			".": "src/index.ts",
			"./client": "src/client.ts",
			"./connector": "src/connector.ts",
			"./message-port": "src/message-port.ts",
			"./node": "src/node-message-port.ts",
			"./server": "src/server.ts",
			"./websocket/client": "src/websocket/client.ts",
			"./websocket/protocol": "src/websocket/protocol.ts",
			"./websocket/server": "src/websocket/server.ts",
			"./wire": "src/wire.ts",
		},
		name: "@artisan/transport",
		ships_in_editor: true,
	},
	{
		directory: "modules/dev-tui",
		entries: {
			".": "src/index.ts",
			"./entry": "src/entry.ts",
			"./model": "src/model.ts",
		},
		name: "@artisan/dev-tui",
		ships_in_editor: false,
	},
	{
		directory: "modules/checklist",
		entries: {
			".": "src/index.ts",
			"./entry": "src/entry.ts",
			"./model": "src/model.ts",
		},
		name: "@artisanstreet/checklist",
		ships_in_editor: false,
	},
];

/** `.` becomes `index`, `./websocket/client` becomes `websocket/client`. */
export const entry_output_name = (subpath: string): string =>
	subpath === "." ? "index" : subpath.replace(/^\.\//u, "");

export const entry_output_path = (subpath: string): string =>
	`./.dist/${entry_output_name(subpath)}.mjs`;

/**
 * Source stays the type entry and the `development` target, so type-checking
 * and every dev or test runner keep reading the real files while anything that
 * resolves normally — the packaged editor's Vite and Rolldown builds — loads
 * the bundle.
 */
export const module_exports = (module: BundledModule): Record<string, unknown> =>
	Object.fromEntries(
		Object.entries(module.entries).map(([subpath, file]) => [
			subpath,
			module.ships_in_editor
				? {
						types: `./${file}`,
						development: `./${file}`,
						default: entry_output_path(subpath),
					}
				: `./${file}`,
		]),
	);

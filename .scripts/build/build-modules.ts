import { isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { Checklist, type Step } from "@artisanstreet/checklist";
import {
	NodeChildProcessSpawner,
	NodeFileSystem,
	NodePath,
	NodeRuntime,
} from "@effect/platform-node-shared";
import { Effect, Layer } from "effect";
import { build } from "rolldown";

import {
	bundled_modules,
	entry_output_name,
	repository_root,
	type BundledModule,
} from "./modules.ts";

/**
 * Bundles each workspace library into its own artifact. Sibling packages and
 * everything from node_modules stay external, so the modules have no build
 * order between them and the application bundles that inline them are not
 * handed eleven copies of Effect.
 */

const is_external = (source: string): boolean => !source.startsWith(".") && !isAbsolute(source);

export const build_module = async (module: BundledModule): Promise<void> => {
	const module_root = resolve(repository_root, module.directory);

	await build({
		external: is_external,
		input: Object.fromEntries(
			Object.entries({ ...module.entries, ...module.internal_entries }).map(
				([subpath, file]) => [entry_output_name(subpath), resolve(module_root, file)],
			),
		),
		output: {
			chunkFileNames: "shared-[hash].mjs",
			cleanDir: true,
			dir: resolve(module_root, ".dist"),
			entryFileNames: "[name].mjs",
			format: "es",
		},
		platform: "node",
		/** Nothing internal should resolve to a sibling's bundle mid-build. */
		resolve: { conditionNames: ["development", "import", "default"] },
	});
};

/**
 * One row per module. They are genuinely independent — every cross-module
 * import is external — so this is the one place in the release loop where
 * overlapping the work cannot change the result.
 */
export const module_build_steps = (): ReadonlyArray<Step> =>
	bundled_modules.map((module) => ({
		name: module.name,
		run: async (step) => {
			await build_module(module);
			step.detail(
				`${Object.keys(module.entries).length + Object.keys(module.internal_entries ?? {}).length} entries`,
			);
		},
	}));

export const ModuleBuildProgram = Checklist.make(
	[{ concurrency: 4, name: "modules", steps: module_build_steps() }],
	{ subtitle: `${bundled_modules.length} packages`, title: "build modules" },
);

const NodeProcessLive = NodeChildProcessSpawner.layer.pipe(
	Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
);

if (process.argv[1] === fileURLToPath(import.meta.url)) {
	NodeRuntime.runMain(ModuleBuildProgram.pipe(Effect.provide(NodeProcessLive)));
}

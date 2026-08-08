import { createRequire } from "node:module";
import { resolve } from "node:path";
import { isSea } from "node:sea";

import { NodeCrypto, NodeFileSystem, NodePath, NodeRuntime } from "@effect/platform-node-shared";
import { WindowsProcessHostModeArgument, WindowsProcessHostProgram } from "@artisan/engines";
import { MakeSnowflakeIdLive } from "@artisan/protocol";
import { Console, Data, Effect, FileSystem, Layer, Schema } from "effect";

import { StartForgeFromEnvironment } from "./entry";
import {
	ForgeSeaRuntimeSmokeModeArgument,
	ForgeSeaSmokeModeArgument,
	ResolveForgeSeaCacheRoot,
} from "./executable-runtime";
import { NodeSeaAssetSourceLive, SeaAssetSource } from "./sea/asset-source";
import { SeaAssetManifest } from "./sea/manifest";
import { make_sea_asset_materializer_layer, SeaAssetMaterializer } from "./sea/materializer";

const SeaManifestAssetId = "artisan-sea-manifest";

class ForgeSeaBootstrapFailure extends Data.TaggedError("ForgeSeaBootstrapFailure")<{
	readonly cause: unknown;
	readonly operation: "environment" | "manifest" | "native-runtime";
}> {}

const DecodeManifest = (bytes: Uint8Array) =>
	Schema.decodeUnknownEffect(Schema.fromJsonString(Schema.Unknown))(
		new TextDecoder().decode(bytes),
	).pipe(
		Effect.mapError((cause) => new ForgeSeaBootstrapFailure({ cause, operation: "manifest" })),
	);

const PrepareSeaRuntime = Effect.gen(function* () {
	const source = yield* SeaAssetSource;
	const materializer = yield* SeaAssetMaterializer;
	const manifest = yield* source.Read(SeaManifestAssetId).pipe(Effect.flatMap(DecodeManifest));
	const materialized = yield* materializer.Materialize(manifest);
	const native_runtime = resolve(materialized.root, "native-runtime", "node_modules");
	const migrations = resolve(materialized.root, "migrations");

	yield* Effect.try({
		try: () => {
			process.env.ARTISAN_MIGRATIONS_PATH = migrations;
			process.env.ARTISAN_NATIVE_RUNTIME = native_runtime;
			process.env.ARTISAN_SEA_RUNTIME_ROOT = materialized.root;
			process.env.NODE_PATH = native_runtime;
		},
		catch: (cause) => new ForgeSeaBootstrapFailure({ cause, operation: "environment" }),
	});
});

const SeaRuntimeLive = make_sea_asset_materializer_layer({
	cache_root: ResolveForgeSeaCacheRoot(process.env),
}).pipe(
	Layer.provideMerge(NodeSeaAssetSourceLive),
	Layer.provideMerge(NodeCrypto.layer),
	Layer.provideMerge(NodeFileSystem.layer),
	Layer.provideMerge(NodePath.layer),
);

const ForgeProgram = PrepareSeaRuntime.pipe(
	Effect.andThen(StartForgeFromEnvironment),
	Effect.provide(SeaRuntimeLive),
	Effect.provide(MakeSnowflakeIdLive(3).pipe(Layer.orDie)),
);

const SeaSmokeProgram = Effect.gen(function* () {
	const source = yield* SeaAssetSource;
	const manifest = yield* source
		.Read(SeaManifestAssetId)
		.pipe(
			Effect.flatMap(DecodeManifest),
			Effect.flatMap(Schema.decodeUnknownEffect(SeaAssetManifest)),
		);

	yield* Console.log(
		JSON.stringify({
			assets: manifest.assets.length,
			manifest_version: manifest.version,
			sea: isSea(),
		}),
	);
}).pipe(Effect.provide(NodeSeaAssetSourceLive));

const SeaRuntimeSmokeProgram = Effect.gen(function* () {
	yield* PrepareSeaRuntime;
	const file_system = yield* FileSystem.FileSystem;
	const native_runtime = process.env.ARTISAN_NATIVE_RUNTIME;
	const migrations = process.env.ARTISAN_MIGRATIONS_PATH;
	if (native_runtime === undefined || migrations === undefined) {
		return yield* new ForgeSeaBootstrapFailure({
			cause: "The SEA runtime environment was not prepared",
			operation: "native-runtime",
		});
	}
	const loaded = yield* Effect.try({
		try: () => {
			const require = createRequire(process.execPath);
			const koffi = require(resolve(native_runtime, "koffi")) as { readonly load?: unknown };
			const node_pty = require(resolve(native_runtime, "node-pty")) as {
				readonly spawn?: unknown;
			};
			if (typeof koffi.load !== "function" || typeof node_pty.spawn !== "function") {
				throw new Error("The extracted native modules did not expose their runtime APIs");
			}

			return { koffi: true as const, node_pty: true as const };
		},
		catch: (cause) => new ForgeSeaBootstrapFailure({ cause, operation: "native-runtime" }),
	});
	const claude_cli = yield* file_system
		.exists(resolve(native_runtime, "claude-agent-sdk", "claude.exe"))
		.pipe(
			Effect.mapError(
				(cause) => new ForgeSeaBootstrapFailure({ cause, operation: "native-runtime" }),
			),
		);
	const migration_files = yield* file_system
		.readDirectory(migrations, { recursive: true })
		.pipe(
			Effect.mapError(
				(cause) => new ForgeSeaBootstrapFailure({ cause, operation: "native-runtime" }),
			),
		);
	if (!claude_cli || !migration_files.some((path) => path.endsWith(".sql"))) {
		return yield* new ForgeSeaBootstrapFailure({
			cause: "The extracted Claude CLI or migrations are missing",
			operation: "native-runtime",
		});
	}
	yield* Console.log(
		JSON.stringify({
			claude_cli: true,
			koffi: loaded.koffi,
			migrations: migration_files.filter((path) => path.endsWith(".sql")).length,
			node_pty: loaded.node_pty,
			runtime: true,
			sea: isSea(),
		}),
	);
}).pipe(Effect.provide(SeaRuntimeLive));

const program = process.argv.includes(WindowsProcessHostModeArgument)
	? WindowsProcessHostProgram
	: process.argv.includes(ForgeSeaRuntimeSmokeModeArgument)
		? SeaRuntimeSmokeProgram
		: process.argv.includes(ForgeSeaSmokeModeArgument)
			? SeaSmokeProgram
			: ForgeProgram;

NodeRuntime.runMain(program);

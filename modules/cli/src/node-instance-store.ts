import { randomUUID } from "node:crypto";

import { NodeChildProcessSpawner, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import {
	make_private_file_permissions_layer,
	PrivateFilePermissions,
	PrivateFilePermissionsPlatform,
} from "@artisan/backend";
import { Console, Effect, FileSystem, Layer, Option, Path, Schema } from "effect";

import { CliPlatform, make_cli_platform_layer } from "./adapters";
import {
	DecodeForgeInstanceConfig,
	ForgeInstanceError,
	ForgeInstanceStore,
	ForgeRuntimeState,
	ForgeSecrets,
	type ForgeInstancePaths,
	GenerateForgeToken,
} from "./instance";

const DecodeSecrets = Schema.decodeUnknownSync(ForgeSecrets);

const instance_error = (code: ForgeInstanceError["code"]) => new ForgeInstanceError({ code });

/** Rejects links at every directory the CLI owns before an instance file is opened. */
const AssertPrivateDirectory = (file_system: FileSystem.FileSystem, path: string) =>
	file_system.stat(path).pipe(
		Effect.filterOrFail(
			(metadata) => metadata.type === "Directory",
			() => new Error("unsafe Artisan home directory"),
		),
		Effect.asVoid,
	);

const CanonicalDirectory = (file_system: FileSystem.FileSystem, path: string) =>
	Effect.gen(function* () {
		const canonical = yield* file_system.realPath(path);
		const metadata = yield* file_system.stat(canonical);
		if (metadata.type !== "Directory") {
			return yield* Effect.fail(new Error("expected an existing directory"));
		}
		return canonical;
	});

const AssertPrivateRegularFile = (file_system: FileSystem.FileSystem, path: string) =>
	file_system.stat(path).pipe(
		Effect.filterOrFail(
			(metadata) => metadata.type === "File",
			() => new Error("unsafe Forge instance file"),
		),
		Effect.asVoid,
	);

/**
 * Migrates the legacy `profiles/<name>/` layout into the home root. Runs at
 * the store's path-resolution choke point: with exactly one legacy profile its
 * contents move to the home root and the empty tree is removed; with more the
 * home is ambiguous and every command fails until the user deletes all but
 * one. A home that already has a root `config.json` is current and skipped.
 */
const MigrateLegacyProfiles = (
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
	home: string,
) =>
	Effect.gen(function* () {
		const { join, resolve } = path_service;
		const home_directory = resolve(home);
		if (yield* file_system.exists(join(home_directory, "config.json"))) return;
		const profiles_directory = join(home_directory, "profiles");
		if (!(yield* file_system.exists(profiles_directory))) return;
		const entries = yield* file_system.readDirectory(profiles_directory);
		const directories: Array<string> = [];
		for (const entry of entries) {
			const metadata = yield* file_system.stat(join(profiles_directory, entry));
			if (metadata.type === "Directory") directories.push(entry);
		}
		if (directories.length > 1) {
			return yield* Effect.fail(
				new ForgeInstanceError({
					cause: new Error(
						`this Artisan home has multiple legacy Forge profiles (${directories
							.sort()
							.join(", ")}) and Artisan now runs one Forge per home; ` +
							`delete all but one directory under ${profiles_directory} and retry`,
					),
					code: "legacy_profiles",
				}),
			);
		}
		const single = directories[0];
		if (single !== undefined) {
			const source = join(profiles_directory, single);
			for (const entry of yield* file_system.readDirectory(source)) {
				const destination = join(home_directory, entry);
				if (yield* file_system.exists(destination)) {
					return yield* Effect.fail(
						new ForgeInstanceError({
							cause: new Error(
								`cannot migrate legacy Forge profile \`${single}\`: ${destination} already exists`,
							),
							code: "legacy_profiles",
						}),
					);
				}
				yield* file_system.rename(join(source, entry), destination);
			}
			yield* Console.error(
				`migrated legacy Forge profile \`${single}\` into the Artisan home root at ${home_directory}`,
			);
		}
		yield* file_system.remove(profiles_directory, { force: true, recursive: true });
	}).pipe(
		Effect.mapError((cause) =>
			cause instanceof ForgeInstanceError
				? cause
				: new ForgeInstanceError({ cause, code: "invalid" }),
		),
	);

/** Node is needed only for atomic same-directory publication and no-follow directory checks. */
export const make_node_instance_store_layer = (home_override?: string) => {
	const InstanceStoreLive = Layer.effect(
		ForgeInstanceStore,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const platform = yield* CliPlatform;
			const home = home_override ?? platform.home;
			const { dirname, join, resolve } = path_service;
			const home_directory = resolve(home);
			const instance_paths: ForgeInstancePaths = {
				config_path: join(home_directory, "config.json"),
				log_path: join(home_directory, "forge.log"),
				readiness_path: join(home_directory, "ready.json"),
				secrets_path: join(home_directory, "secrets.json"),
				state_path: join(home_directory, "state.json"),
			};
			const Migrated = yield* Effect.cached(
				MigrateLegacyProfiles(file_system, path_service, home),
			);
			const Paths = () => Migrated.pipe(Effect.as(instance_paths));

			const EnsureDirectory = (
				path: string,
				permissions: PrivateFilePermissions["Service"],
			) =>
				file_system.makeDirectory(path, { recursive: true, mode: 0o700 }).pipe(
					Effect.andThen(AssertPrivateDirectory(file_system, path)),
					Effect.andThen(permissions.RestrictDirectory(path)),
					Effect.mapError(() => instance_error("invalid")),
				);

			const EnsureHomeDirectory = (permissions: PrivateFilePermissions["Service"]) =>
				EnsureDirectory(home_directory, permissions).pipe(
					Effect.andThen(Migrated),
					Effect.as(instance_paths),
				);

			const ReadRequired = <A>(path: string, decode: (input: unknown) => A) =>
				file_system.exists(path).pipe(
					Effect.filterOrFail(
						(exists) => exists,
						() => instance_error("missing"),
					),
					Effect.andThen(AssertPrivateRegularFile(file_system, path)),
					Effect.andThen(file_system.readFileString(path, "utf8")),
					Effect.flatMap(Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)),
					Effect.flatMap((decoded) =>
						Effect.try({
							try: () => decode(decoded),
							catch: () => instance_error("invalid"),
						}),
					),
					Effect.mapError((cause) =>
						cause instanceof ForgeInstanceError ? cause : instance_error("invalid"),
					),
				);

			const ExistingPaths = () =>
				Migrated.pipe(
					Effect.andThen(
						AssertPrivateDirectory(file_system, home_directory).pipe(
							Effect.mapError(() => instance_error("invalid")),
						),
					),
					Effect.as(instance_paths),
				);

			const ReadState = (path: string) =>
				AssertPrivateRegularFile(file_system, path).pipe(
					Effect.andThen(file_system.readFileString(path, "utf8")),
					Effect.flatMap(
						Schema.decodeUnknownEffect(Schema.fromJsonString(ForgeRuntimeState)),
					),
					/** A stale, absent, or malformed ownership hint is never actionable. */
					Effect.catch(() => Effect.succeed(undefined)),
				);

			const WriteAtomicPrivate = (
				path: string,
				value: unknown,
				permissions: PrivateFilePermissions["Service"],
			) =>
				Effect.gen(function* () {
					/** Decode before serialization so even internal callers cannot write invalid state. */
					const encoded = JSON.stringify(value);
					if (encoded === undefined) return yield* Effect.fail(instance_error("invalid"));
					const temporary = join(dirname(path), `.${randomUUID()}.tmp`);
					const created = yield* permissions
						.CreatePrivate(temporary)
						.pipe(Effect.mapError(() => instance_error("invalid")));
					if (Option.isNone(created))
						return yield* Effect.fail(instance_error("invalid"));
					yield* file_system
						.writeFileString(temporary, `${encoded}\n`, { flag: "w" })
						.pipe(Effect.mapError(() => instance_error("invalid")));
					yield* permissions
						.RestrictOwned(temporary, created.value)
						.pipe(Effect.mapError(() => instance_error("invalid")));
					yield* file_system.rename(temporary, path).pipe(
						Effect.mapError(() => instance_error("invalid")),
						Effect.ensuring(
							file_system.remove(temporary, { force: true }).pipe(Effect.ignore),
						),
					);
					yield* permissions
						.Restrict(path)
						.pipe(Effect.mapError(() => instance_error("invalid")));
				});

			const permissions = yield* PrivateFilePermissions;
			return ForgeInstanceStore.of({
				Ensure: (input) =>
					Effect.gen(function* () {
						const paths = yield* EnsureHomeDirectory(permissions);
						const config = yield* Effect.try({
							catch: () => instance_error("invalid"),
							try: () => DecodeForgeInstanceConfig(input),
						});
						yield* EnsureDirectory(config.data_root, permissions);
						const data_root = yield* CanonicalDirectory(
							file_system,
							config.data_root,
						).pipe(Effect.mapError(() => instance_error("invalid")));
						const normalized = yield* Effect.try({
							catch: () => instance_error("invalid"),
							try: () =>
								DecodeForgeInstanceConfig({
									...config,
									data_root,
								}),
						});
						yield* WriteAtomicPrivate(paths.config_path, normalized, permissions);
						const existing = yield* ReadRequired(
							paths.secrets_path,
							DecodeSecrets,
						).pipe(
							Effect.map(Option.some),
							Effect.catch((error) =>
								error.code === "missing"
									? Effect.succeed(Option.none())
									: Effect.fail(error),
							),
						);
						if (Option.isNone(existing))
							yield* WriteAtomicPrivate(
								paths.secrets_path,
								yield* Effect.try({
									catch: () => instance_error("invalid"),
									try: () =>
										DecodeSecrets({
											auth_token: GenerateForgeToken(),
											version: 1,
										}),
								}),
								permissions,
							);
					}),
				Load: () =>
					Effect.gen(function* () {
						const paths = yield* ExistingPaths();
						return yield* ReadRequired(paths.config_path, DecodeForgeInstanceConfig);
					}),
				LoadSecrets: () =>
					Effect.gen(function* () {
						const paths = yield* ExistingPaths();
						return yield* ReadRequired(paths.secrets_path, DecodeSecrets);
					}),
				ReadState: () =>
					Effect.gen(function* () {
						const paths = yield* ExistingPaths();
						return yield* ReadState(paths.state_path);
					}),
				RemoveStateIfOwned: (expected_instance_id) =>
					Effect.gen(function* () {
						const paths = yield* ExistingPaths();
						const state = yield* ReadState(paths.state_path);
						if (state?.instance_id !== expected_instance_id) return false;
						yield* file_system
							.remove(paths.state_path, { force: true })
							.pipe(Effect.mapError(() => instance_error("invalid")));
						return true;
					}),
				Paths,
			});
		}),
	);

	const node_process = NodeChildProcessSpawner.layer.pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(NodePath.layer),
	);
	const platform = make_cli_platform_layer().pipe(Layer.provide(NodePath.layer));
	const platform_services = Layer.mergeAll(NodeFileSystem.layer, NodePath.layer, platform);
	const selected_permissions = Layer.unwrap(
		Effect.gen(function* () {
			const host = yield* CliPlatform;
			return make_private_file_permissions_layer.pipe(
				Layer.provideMerge(NodeFileSystem.layer),
				Layer.provideMerge(node_process),
				Layer.provideMerge(
					Layer.succeed(PrivateFilePermissionsPlatform, {
						kind: host.kind === "win32" ? "win32" : "posix",
					}),
				),
			);
		}),
	).pipe(Layer.provide(platform_services));
	return InstanceStoreLive.pipe(
		Layer.provideMerge(selected_permissions),
		Layer.provideMerge(platform_services),
	);
};

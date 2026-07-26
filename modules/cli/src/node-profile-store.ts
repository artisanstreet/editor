import { randomUUID } from "node:crypto";

import { NodeChildProcessSpawner, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import {
	make_private_file_permissions_layer,
	PrivateFilePermissions,
	PrivateFilePermissionsPlatform,
} from "@artisan/backend";
import { Effect, FileSystem, Layer, Option, Path, Schema } from "effect";

import { CliPlatform, make_cli_platform_layer } from "./adapters";
import {
	DecodeForgeProfileConfig,
	ForgeProfileError,
	ForgeProfileName,
	ForgeProfileStore,
	ForgeRuntimeState,
	ForgeSecrets,
	type ForgeProfilePaths,
	GenerateForgeToken,
} from "./profile";

const DecodeProfile = Schema.decodeUnknownSync(ForgeProfileName);
const DecodeSecrets = Schema.decodeUnknownSync(ForgeSecrets);
const DecodeState = Schema.decodeUnknownSync(ForgeRuntimeState);

const profile_error = (code: ForgeProfileError["code"]) => new ForgeProfileError({ code });

/** Rejects links at every directory the CLI owns before a profile file is opened. */
const AssertPrivateDirectory = (file_system: FileSystem.FileSystem, path: string) =>
	file_system.stat(path).pipe(
		Effect.filterOrFail(
			(metadata) => metadata.type === "Directory",
			() => new Error("unsafe profile directory"),
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
			() => new Error("unsafe profile file"),
		),
		Effect.asVoid,
	);

/** Node is needed only for atomic same-directory publication and no-follow directory checks. */
export const make_node_profile_store_layer = (home_override?: string) => {
	const ProfileStoreLive = Layer.effect(
		ForgeProfileStore,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const platform = yield* CliPlatform;
			const home = home_override ?? platform.home;
			const { dirname, join, resolve } = path_service;
			const Paths = (profile: string): Effect.Effect<ForgeProfilePaths, ForgeProfileError> =>
				Effect.try({
					catch: () => profile_error("invalid"),
					try: () => {
						const name = DecodeProfile(profile);
						const profile_directory = resolve(home, "profiles", name);
						const profiles_directory = resolve(home, "profiles");
						if (dirname(profile_directory) !== profiles_directory)
							throw new Error("profile escaped home");
						return {
							config_path: join(profile_directory, "config.json"),
							log_path: join(profile_directory, "forge.log"),
							readiness_path: join(profile_directory, "ready.json"),
							secrets_path: join(profile_directory, "secrets.json"),
							state_path: join(profile_directory, "state.json"),
						};
					},
				});

			const EnsureDirectory = (
				path: string,
				permissions: PrivateFilePermissions["Service"],
			) =>
				file_system.makeDirectory(path, { recursive: true, mode: 0o700 }).pipe(
					Effect.andThen(AssertPrivateDirectory(file_system, path)),
					Effect.andThen(permissions.RestrictDirectory(path)),
					Effect.mapError(() => profile_error("invalid")),
				);

			const EnsureProfileDirectory = (
				profile: string,
				permissions: PrivateFilePermissions["Service"],
			) =>
				Effect.gen(function* () {
					const paths = yield* Paths(profile);
					const home_directory = resolve(home);
					yield* EnsureDirectory(home_directory, permissions);
					yield* EnsureDirectory(dirname(dirname(paths.config_path)), permissions);
					yield* EnsureDirectory(dirname(paths.config_path), permissions);
					return paths;
				});

			const ReadRequired = <A>(path: string, decode: (input: unknown) => A) =>
				file_system.exists(path).pipe(
					Effect.filterOrFail(
						(exists) => exists,
						() => profile_error("missing"),
					),
					Effect.andThen(AssertPrivateRegularFile(file_system, path)),
					Effect.andThen(file_system.readFileString(path, "utf8")),
					Effect.flatMap((encoded) =>
						Effect.try({
							try: () => decode(JSON.parse(encoded)),
							catch: () => profile_error("invalid"),
						}),
					),
					Effect.mapError((cause) =>
						cause instanceof ForgeProfileError ? cause : profile_error("invalid"),
					),
				);

			const ExistingPaths = (profile: string) =>
				Effect.gen(function* () {
					const paths = yield* Paths(profile);
					yield* Effect.all([
						AssertPrivateDirectory(file_system, resolve(home)),
						AssertPrivateDirectory(file_system, dirname(dirname(paths.config_path))),
						AssertPrivateDirectory(file_system, dirname(paths.config_path)),
					]).pipe(Effect.mapError(() => profile_error("invalid")));
					return paths;
				});

			const ReadState = (path: string) =>
				AssertPrivateRegularFile(file_system, path).pipe(
					Effect.andThen(file_system.readFileString(path, "utf8")),
					Effect.flatMap((encoded) =>
						Effect.try({
							try: () => DecodeState(JSON.parse(encoded)),
							catch: () => profile_error("invalid"),
						}),
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
					if (encoded === undefined) return yield* Effect.fail(profile_error("invalid"));
					const temporary = join(dirname(path), `.${randomUUID()}.tmp`);
					const created = yield* permissions
						.CreatePrivate(temporary)
						.pipe(Effect.mapError(() => profile_error("invalid")));
					if (Option.isNone(created)) return yield* Effect.fail(profile_error("invalid"));
					yield* file_system
						.writeFileString(temporary, `${encoded}\n`, { flag: "w" })
						.pipe(Effect.mapError(() => profile_error("invalid")));
					yield* permissions
						.RestrictOwned(temporary, created.value)
						.pipe(Effect.mapError(() => profile_error("invalid")));
					yield* file_system.rename(temporary, path).pipe(
						Effect.mapError(() => profile_error("invalid")),
						Effect.ensuring(
							file_system.remove(temporary, { force: true }).pipe(Effect.ignore),
						),
					);
					yield* permissions
						.Restrict(path)
						.pipe(Effect.mapError(() => profile_error("invalid")));
				});

			const permissions = yield* PrivateFilePermissions;
			return ForgeProfileStore.of({
				Ensure: (profile, input) =>
					Effect.gen(function* () {
						const paths = yield* EnsureProfileDirectory(profile, permissions);
						const config = yield* Effect.try({
							catch: () => profile_error("invalid"),
							try: () => DecodeForgeProfileConfig(input),
						});
						yield* EnsureDirectory(config.data_root, permissions);
						const data_root = yield* CanonicalDirectory(
							file_system,
							config.data_root,
						).pipe(Effect.mapError(() => profile_error("invalid")));
						const project_roots = yield* Effect.forEach(config.project_roots, (root) =>
							CanonicalDirectory(file_system, root).pipe(
								Effect.mapError(() => profile_error("invalid")),
							),
						);
						const normalized = yield* Effect.try({
							catch: () => profile_error("invalid"),
							try: () =>
								DecodeForgeProfileConfig({
									...config,
									data_root,
									project_roots: [...new Set(project_roots)],
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
									catch: () => profile_error("invalid"),
									try: () =>
										DecodeSecrets({
											auth_token: GenerateForgeToken(),
											version: 1,
										}),
								}),
								permissions,
							);
					}),
				Load: (profile) =>
					Effect.gen(function* () {
						const paths = yield* ExistingPaths(profile);
						return yield* ReadRequired(paths.config_path, DecodeForgeProfileConfig);
					}),
				LoadSecrets: (profile) =>
					Effect.gen(function* () {
						const paths = yield* ExistingPaths(profile);
						return yield* ReadRequired(paths.secrets_path, DecodeSecrets);
					}),
				ReadState: (profile) =>
					Effect.gen(function* () {
						const paths = yield* ExistingPaths(profile);
						return yield* ReadState(paths.state_path);
					}),
				RemoveStateIfOwned: (profile, expected_instance_id) =>
					Effect.gen(function* () {
						const paths = yield* ExistingPaths(profile);
						const state = yield* ReadState(paths.state_path);
						if (
							state?.profile !== DecodeProfile(profile) ||
							state.instance_id !== expected_instance_id
						)
							return false;
						yield* file_system
							.remove(paths.state_path, { force: true })
							.pipe(Effect.mapError(() => profile_error("invalid")));
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
	return ProfileStoreLive.pipe(
		Layer.provideMerge(selected_permissions),
		Layer.provideMerge(platform_services),
	);
};

import { Context, Data, Effect, FileSystem, Layer, Path, Schema, Semaphore } from "effect";

import {
	InstallationManifest,
	type ActivatedInstallationManifest,
	type InstallationManifest as InstallationManifestValue,
} from "./installation-manifest";

export type InstallationState =
	| { readonly _tag: "Absent" }
	| { readonly _tag: "Healthy"; readonly manifest: ActivatedInstallationManifest }
	| { readonly _tag: "Partial"; readonly manifest: InstallationManifestValue }
	| {
			readonly _tag: "Malformed";
			readonly cause: unknown;
			readonly manifest_path: string;
	  };

export class InstallationStoreFailure extends Data.TaggedError("InstallationStoreFailure")<{
	readonly cause: unknown;
	readonly operation: "inspect" | "write";
	readonly path: string;
}> {}

export class InstallationRootMismatch extends Data.TaggedError("InstallationRootMismatch")<{
	readonly actual_root: string;
	readonly expected_root: string;
}> {}

export class InstallationStore extends Context.Service<
	InstallationStore,
	{
		readonly Inspect: () => Effect.Effect<InstallationState, InstallationStoreFailure>;
		readonly WriteAtomic: (
			manifest: InstallationManifestValue,
		) => Effect.Effect<void, InstallationStoreFailure | InstallationRootMismatch>;
	}
>()("Artisan/Distribution/InstallationStore") {}

const is_not_found = (cause: unknown) =>
	typeof cause === "object" &&
	cause !== null &&
	"reason" in cause &&
	typeof cause.reason === "object" &&
	cause.reason !== null &&
	"_tag" in cause.reason &&
	cause.reason._tag === "NotFound";

const RemoveTemporary = (file_system: FileSystem.FileSystem, temporary_path: string) =>
	file_system.remove(temporary_path).pipe(Effect.ignore);

/** Creates an installation store confined to the explicitly supplied product root. */
export const make_installation_store_layer = (installation_root: string) =>
	Layer.effect(
		InstallationStore,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const write_lock = yield* Semaphore.make(1);
			const manifest_path = path_service.join(installation_root, "installation.json");
			const temporary_path = path_service.join(installation_root, ".installation.json.tmp");

			const Inspect = () =>
				file_system.readFileString(manifest_path).pipe(
					Effect.flatMap((content) =>
						Effect.try({
							try: () => JSON.parse(content) as unknown,
							catch: (cause) => cause,
						}).pipe(
							Effect.flatMap(Schema.decodeUnknownEffect(InstallationManifest)),
							Effect.match({
								onFailure: (cause): InstallationState => ({
									_tag: "Malformed",
									cause,
									manifest_path,
								}),
								onSuccess: (decoded): InstallationState => {
									const manifest =
										decoded.activation_state === "active" &&
										decoded.finalization_state === undefined
											? {
													...decoded,
													finalization_state: "pending" as const,
												}
											: decoded;
									return manifest.activation_state === "active" &&
										manifest.transaction.state === "idle" &&
										manifest.finalization_state === "complete"
										? { _tag: "Healthy", manifest }
										: { _tag: "Partial", manifest };
								},
							}),
						),
					),
					Effect.catch((cause) =>
						is_not_found(cause)
							? Effect.succeed<InstallationState>({ _tag: "Absent" })
							: Effect.fail(
									new InstallationStoreFailure({
										cause,
										operation: "inspect",
										path: manifest_path,
									}),
								),
					),
				);

			const WriteAtomic = (manifest: InstallationManifestValue) =>
				write_lock
					.withPermits(1)(
						Effect.gen(function* () {
							const validated =
								yield* Schema.decodeUnknownEffect(InstallationManifest)(manifest);

							if (
								path_service.resolve(validated.install_root) !==
								path_service.resolve(installation_root)
							) {
								return yield* new InstallationRootMismatch({
									actual_root: validated.install_root,
									expected_root: installation_root,
								});
							}

							yield* file_system.makeDirectory(installation_root, {
								recursive: true,
							});
							yield* Effect.scoped(
								Effect.gen(function* () {
									const file = yield* file_system.open(temporary_path, {
										flag: "w",
									});
									yield* file.writeAll(
										new TextEncoder().encode(
											`${JSON.stringify(validated, undefined, "\t")}\n`,
										),
									);
									yield* file.sync;
								}),
							);
							yield* file_system.rename(temporary_path, manifest_path);
						}),
					)
					.pipe(
						Effect.ensuring(RemoveTemporary(file_system, temporary_path)),
						Effect.mapError((cause) =>
							cause instanceof InstallationRootMismatch
								? cause
								: new InstallationStoreFailure({
										cause,
										operation: "write",
										path: manifest_path,
									}),
						),
					);

			return { Inspect, WriteAtomic };
		}),
	);

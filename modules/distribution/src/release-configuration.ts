import { Context, Data, Effect, FileSystem, Layer, Path, Schema, Semaphore } from "effect";

import { ReleaseChannel } from "./common";

export const InstalledReleaseConfiguration = Schema.Struct({
	format_version: Schema.Literal(1),
	owner: Schema.NonEmptyString,
	repository: Schema.NonEmptyString,
	channel: ReleaseChannel,
	signing_key_id: Schema.NonEmptyString,
	signing_public_key_base64: Schema.NonEmptyString,
});
export type InstalledReleaseConfiguration = typeof InstalledReleaseConfiguration.Type;

export type InstalledReleaseConfigurationState =
	| { readonly _tag: "Absent" }
	| {
			readonly _tag: "Available";
			readonly configuration: InstalledReleaseConfiguration;
	  }
	| {
			readonly _tag: "Malformed";
			readonly cause: unknown;
			readonly configuration_path: string;
	  };

export class InstalledReleaseConfigurationFailure extends Data.TaggedError(
	"InstalledReleaseConfigurationFailure",
)<{
	readonly cause: unknown;
	readonly operation: "inspect" | "write";
	readonly path: string;
}> {}

export class InstalledReleaseConfigurationStore extends Context.Service<
	InstalledReleaseConfigurationStore,
	{
		readonly Inspect: () => Effect.Effect<
			InstalledReleaseConfigurationState,
			InstalledReleaseConfigurationFailure
		>;
		readonly WriteAtomic: (
			configuration: InstalledReleaseConfiguration,
		) => Effect.Effect<void, InstalledReleaseConfigurationFailure>;
	}
>()("Artisan/Distribution/InstalledReleaseConfigurationStore") {}

const IsNotFound = (cause: unknown) =>
	typeof cause === "object" &&
	cause !== null &&
	"reason" in cause &&
	typeof cause.reason === "object" &&
	cause.reason !== null &&
	"_tag" in cause.reason &&
	cause.reason._tag === "NotFound";

/** Stores only public release origin and trust material needed by permanent ae. */
export const make_installed_release_configuration_store_layer = (installation_root: string) =>
	Layer.effect(
		InstalledReleaseConfigurationStore,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const write_lock = yield* Semaphore.make(1);
			const configuration_path = path_service.join(installation_root, "distribution.json");
			const temporary_path = path_service.join(installation_root, ".distribution.json.tmp");

			const Inspect = () =>
				file_system.readFileString(configuration_path).pipe(
					Effect.flatMap((content) =>
						Effect.try({
							try: () => JSON.parse(content) as unknown,
							catch: (cause) => cause,
						}).pipe(
							Effect.flatMap(
								Schema.decodeUnknownEffect(InstalledReleaseConfiguration),
							),
							Effect.match({
								onFailure: (cause): InstalledReleaseConfigurationState => ({
									_tag: "Malformed",
									cause,
									configuration_path,
								}),
								onSuccess: (configuration): InstalledReleaseConfigurationState => ({
									_tag: "Available",
									configuration,
								}),
							}),
						),
					),
					Effect.catch((cause) =>
						IsNotFound(cause)
							? Effect.succeed<InstalledReleaseConfigurationState>({
									_tag: "Absent",
								})
							: Effect.fail(
									new InstalledReleaseConfigurationFailure({
										cause,
										operation: "inspect",
										path: configuration_path,
									}),
								),
					),
				);

			const WriteAtomic = (configuration: InstalledReleaseConfiguration) =>
				write_lock
					.withPermits(1)(
						Effect.gen(function* () {
							const validated = yield* Schema.decodeUnknownEffect(
								InstalledReleaseConfiguration,
							)(configuration);
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
							yield* file_system.rename(temporary_path, configuration_path);
						}),
					)
					.pipe(
						Effect.ensuring(
							file_system.remove(temporary_path, { force: true }).pipe(Effect.ignore),
						),
						Effect.mapError(
							(cause) =>
								new InstalledReleaseConfigurationFailure({
									cause,
									operation: "write",
									path: configuration_path,
								}),
						),
					);

			return InstalledReleaseConfigurationStore.of({ Inspect, WriteAtomic });
		}),
	);

import { dirname, resolve } from "node:path";

import { Config, Context, Data, Effect, FileSystem, Layer, Option, Stream } from "effect";
import { ChildProcess } from "effect/unstable/process";
import { ChildProcessSpawner } from "effect/unstable/process/ChildProcessSpawner";

import { CliPlatform } from "./adapters";

const protocol_key = "HKCU\\Software\\Classes\\artisan";
const protocol_command_key = `${protocol_key}\\shell\\open\\command`;

export type ForgeProtocolHandlerState = "registered" | "missing" | "unsupported";

export class ForgeProtocolHandlerError extends Data.TaggedError("ForgeProtocolHandlerError")<{
	readonly cause?: unknown;
	readonly code: "failed";
}> {}

export class ForgeProtocolHandler extends Context.Service<
	ForgeProtocolHandler,
	{
		readonly Configure: () => Effect.Effect<
			void,
			ForgeProtocolHandlerError,
			ChildProcessSpawner
		>;
		readonly Status: () => Effect.Effect<
			ForgeProtocolHandlerState,
			ForgeProtocolHandlerError,
			ChildProcessSpawner
		>;
	}
>()("Artisan/ForgeProtocolHandler") {}

export const ForgeProtocolCommand = (desktop_executable_path: string) =>
	`"${desktop_executable_path}" "%1"`;

const RunRegistry = (arguments_: ReadonlyArray<string>, capture_output = false) =>
	Effect.scoped(
		Effect.gen(function* () {
			const child = yield* ChildProcess.make("reg.exe", arguments_, {
				stderr: "ignore",
				stdin: "ignore",
				stdout: capture_output ? "pipe" : "ignore",
			});
			const output = capture_output
				? yield* child.stdout.pipe(Stream.decodeText(), Stream.mkString)
				: "";
			const exit_code = yield* child.exitCode;
			return { exit_code, output };
		}).pipe(
			Effect.mapError((cause) => new ForgeProtocolHandlerError({ cause, code: "failed" })),
		),
	);

/** Registers only a current-user URL handler; elevation and machine-wide writes are forbidden. */
export const make_forge_protocol_handler_layer = Layer.effect(
	ForgeProtocolHandler,
	Effect.gen(function* () {
		const file_system = yield* FileSystem.FileSystem;
		const platform = yield* CliPlatform;
		const configured_executable = yield* Config.string("ARTISAN_DESKTOP_EXECUTABLE").pipe(
			Config.option,
		);
		const desktop_executable_path = Option.getOrElse(configured_executable, () =>
			Option.match(platform.cli_entry_path, {
				onNone: () => resolve(dirname(platform.executable_path), "Artisan Editor.exe"),
				onSome: (entry_path) =>
					resolve(dirname(entry_path), "..", "..", "Artisan Editor.exe"),
			}),
		);
		const handler_command = ForgeProtocolCommand(desktop_executable_path);
		const executable_available = file_system.stat(desktop_executable_path).pipe(
			Effect.map((metadata) => metadata.type === "File"),
			Effect.orElseSucceed(() => false),
		);

		if (process.platform !== "win32") {
			return ForgeProtocolHandler.of({
				Configure: () => Effect.void,
				Status: () => Effect.succeed("unsupported"),
			});
		}

		const AddValue = (key: string, name: ReadonlyArray<string>, value: string) =>
			RunRegistry(["ADD", key, ...name, "/d", value, "/f"]).pipe(
				Effect.filterOrFail(
					(result) => result.exit_code === 0,
					() => new ForgeProtocolHandlerError({ code: "failed" }),
				),
				Effect.asVoid,
			);

		return ForgeProtocolHandler.of({
			Configure: () =>
				Effect.gen(function* () {
					if (!(yield* executable_available)) {
						return yield* new ForgeProtocolHandlerError({
							cause: new Error("Artisan Editor executable is not installed"),
							code: "failed",
						});
					}
					yield* Effect.all(
						[
							AddValue(protocol_key, ["/ve"], "URL:Artisan Forge"),
							AddValue(protocol_key, ["/v", "URL Protocol"], ""),
							AddValue(protocol_command_key, ["/ve"], handler_command),
						],
						{ concurrency: 1, discard: true },
					);
				}),
			Status: () =>
				Effect.gen(function* () {
					if (!(yield* executable_available)) return "missing" as const;
					const { exit_code, output } = yield* RunRegistry(
						["QUERY", protocol_command_key, "/ve"],
						true,
					);
					return exit_code === 0 && output.includes(handler_command)
						? ("registered" as const)
						: ("missing" as const);
				}),
		});
	}),
);

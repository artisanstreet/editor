import { NodeChildProcessSpawner, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Deferred, Effect, Layer, Option, Ref, Schema, Semaphore, Stream } from "effect";
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process";
import { Buffer } from "node:buffer";

import {
	NativeDirectoryPicker,
	NativeDirectoryPickerError,
	NativeDirectoryPickerUnavailable,
} from "./native-directory-picker";

const maximum_stderr_bytes = 64 * 1024;
const maximum_stdout_bytes = 128 * 1024;

const NativeDirectoryPickerResultSchema = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("cancelled") }),
	Schema.Struct({ kind: Schema.Literal("selected"), path: Schema.NonEmptyString }),
]);

interface CaptureState {
	readonly chunks: ReadonlyArray<Uint8Array>;
	readonly retained_bytes: number;
	readonly total_bytes: number;
}

const InitialCaptureState = (): CaptureState => ({
	chunks: [],
	retained_bytes: 0,
	total_bytes: 0,
});

const AppendCapture = (state: CaptureState, chunk: Uint8Array, limit: number): CaptureState => {
	const total_bytes = Math.min(Number.MAX_SAFE_INTEGER, state.total_bytes + chunk.byteLength);
	const remaining = Math.max(0, limit - state.retained_bytes);
	const retained = remaining === 0 ? undefined : chunk.slice(0, remaining);

	return {
		chunks: retained === undefined ? state.chunks : [...state.chunks, retained],
		retained_bytes: state.retained_bytes + (retained?.byteLength ?? 0),
		total_bytes,
	};
};

const MaterializeCapture = (state: CaptureState) => {
	const bytes = new Uint8Array(state.retained_bytes);
	let offset = 0;
	for (const chunk of state.chunks) {
		bytes.set(chunk, offset);
		offset += chunk.byteLength;
	}
	return bytes;
};

const Capture = <E, R>(
	stream: Stream.Stream<Uint8Array, E, R>,
	limit: number,
	state: Ref.Ref<CaptureState>,
	overflow: Deferred.Deferred<void>,
) =>
	stream.pipe(
		Stream.runForEach((chunk) =>
			Ref.updateAndGet(state, (current) => AppendCapture(current, chunk, limit)).pipe(
				Effect.flatMap((next) =>
					next.total_bytes > next.retained_bytes
						? Deferred.succeed(overflow, undefined).pipe(Effect.asVoid)
						: Effect.void,
				),
			),
		),
	);

const windows_picker_script = [
	"$ErrorActionPreference = 'Stop'",
	"$ProgressPreference = 'SilentlyContinue'",
	"Add-Type -AssemblyName System.Windows.Forms",
	"$dialog = New-Object System.Windows.Forms.FolderBrowserDialog",
	"try {",
	"  $dialog.AutoUpgradeEnabled = $true",
	"  $dialog.Description = 'Select a project folder to attach to Artisan'",
	"  if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK -and -not [String]::IsNullOrWhiteSpace($dialog.SelectedPath)) {",
	"    [Console]::Out.Write(([pscustomobject]@{kind='selected';path=$dialog.SelectedPath}|ConvertTo-Json -Compress))",
	"  } else {",
	'    [Console]::Out.Write(\'{"kind":"cancelled"}\')',
	"  }",
	"} finally {",
	"  $dialog.Dispose()",
	"}",
].join("\n");

/** Creates the fixed, argument-only PowerShell command for the Windows picker. */
export const MakeWindowsNativeDirectoryPickerCommand = () =>
	ChildProcess.make(
		"powershell.exe",
		[
			"-NoLogo",
			"-NoProfile",
			"-NonInteractive",
			"-STA",
			"-WindowStyle",
			"Hidden",
			"-EncodedCommand",
			Buffer.from(windows_picker_script, "utf16le").toString("base64"),
		],
		{
			forceKillAfter: "1 second",
			shell: false,
			stderr: "pipe",
			stdin: "ignore",
			stdout: "pipe",
		},
	);

/** Validates the picker protocol at its single process-output boundary. */
export const DecodeNativeDirectoryPickerResult = (stdout: Uint8Array) =>
	Effect.try({
		try: () => JSON.parse(new TextDecoder().decode(stdout).trim()),
		catch: (cause) => cause,
	}).pipe(
		Effect.flatMap(
			Schema.decodeUnknownEffect(NativeDirectoryPickerResultSchema, {
				onExcessProperty: "error",
			}),
		),
		Effect.mapError(
			(cause) => new NativeDirectoryPickerError({ cause, code: "invalid_output" }),
		),
	);

/** Builds the supported Windows picker over Effect's scoped child-process capability. */
export const make_native_directory_picker_layer = (platform: NodeJS.Platform) => {
	if (platform !== "win32") return NativeDirectoryPickerUnavailable;

	return Layer.effect(
		NativeDirectoryPicker,
		Effect.gen(function* () {
			const spawner = yield* ChildProcessSpawner.ChildProcessSpawner;
			const dialog_gate = yield* Semaphore.make(1);
			const PickUnlocked = Effect.scoped(
				Effect.gen(function* () {
					const handle = yield* spawner.spawn(MakeWindowsNativeDirectoryPickerCommand());
					const overflow = yield* Deferred.make<void>();
					const stdout_state = yield* Ref.make(InitialCaptureState());
					const stderr_state = yield* Ref.make(InitialCaptureState());
					const completed = Effect.all(
						[
							Capture(handle.stdout, maximum_stdout_bytes, stdout_state, overflow),
							Capture(handle.stderr, maximum_stderr_bytes, stderr_state, overflow),
							handle.exitCode,
						] as const,
						{ concurrency: "unbounded" },
					).pipe(Effect.map(([, , exit_code]) => ({ type: "exit" as const, exit_code })));
					const outcome = yield* Effect.raceFirst(
						Effect.raceFirst(
							completed,
							Deferred.await(overflow).pipe(
								Effect.as({ type: "invalid_output" as const }),
							),
						),
						Effect.sleep("5 minutes").pipe(Effect.as({ type: "timeout" as const })),
					);

					if (outcome.type === "timeout") {
						return yield* Effect.fail(
							new NativeDirectoryPickerError({ code: "timeout" }),
						);
					}
					if (outcome.type === "invalid_output") {
						return yield* Effect.fail(
							new NativeDirectoryPickerError({ code: "invalid_output" }),
						);
					}
					const [stdout_capture, stderr_capture] = yield* Effect.all([
						Ref.get(stdout_state),
						Ref.get(stderr_state),
					]);
					if (
						stdout_capture.total_bytes > stdout_capture.retained_bytes ||
						stderr_capture.total_bytes > stderr_capture.retained_bytes
					) {
						return yield* Effect.fail(
							new NativeDirectoryPickerError({ code: "invalid_output" }),
						);
					}
					if (outcome.exit_code !== 0) {
						return yield* Effect.fail(
							new NativeDirectoryPickerError({ code: "process_failed" }),
						);
					}

					return yield* DecodeNativeDirectoryPickerResult(
						MaterializeCapture(stdout_capture),
					);
				}),
			).pipe(
				Effect.mapError((cause) =>
					cause instanceof NativeDirectoryPickerError
						? cause
						: new NativeDirectoryPickerError({ cause, code: "process_failed" }),
				),
			);
			const Pick = () =>
				Semaphore.withPermitsIfAvailable(
					dialog_gate,
					1,
				)(PickUnlocked).pipe(
					Effect.flatMap(
						Option.match({
							onNone: () =>
								Effect.fail(new NativeDirectoryPickerError({ code: "busy" })),
							onSome: Effect.succeed,
						}),
					),
				);

			return { Pick };
		}),
	);
};

/** Builds the production Node picker from Effect's Node child-process platform layer. */
export const make_node_native_directory_picker_layer = (platform: NodeJS.Platform) =>
	make_native_directory_picker_layer(platform).pipe(
		Layer.provide(
			NodeChildProcessSpawner.layer.pipe(
				Layer.provideMerge(NodeFileSystem.layer),
				Layer.provideMerge(NodePath.layer),
			),
		),
	);

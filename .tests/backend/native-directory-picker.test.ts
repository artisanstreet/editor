import { Clock, Effect, Exit, Fiber, Layer, Sink, Stream } from "effect";
import { TestClock } from "effect/testing";
import { spawnSync } from "node:child_process";
import {
	ChildProcessSpawner,
	ExitCode,
	make as make_spawner,
	makeHandle,
	ProcessId,
} from "effect/unstable/process/ChildProcessSpawner";
import { describe, expect, it } from "vitest";

import {
	NativeDirectoryPicker,
	NativeDirectoryPickerError,
	NativeDirectoryPickerUnavailable,
} from "../../modules/backend/src/projects/native-directory-picker";
import {
	DecodeNativeDirectoryPickerResult,
	MakeWindowsNativeDirectoryPickerCommand,
	MakeWindowsNativeDirectoryPickerProbeCommand,
	make_native_directory_picker_layer,
} from "../../modules/backend/src/projects/node-native-directory-picker";

const Pick = Effect.gen(function* () {
	const picker = yield* NativeDirectoryPicker;
	return yield* picker.Pick();
});

const MakeProcessLayer = ({
	exit_code = 0,
	on_finalize,
	stderr = new Uint8Array(),
	stdout,
}: {
	readonly exit_code?: number;
	readonly on_finalize?: () => void;
	readonly stderr?: Uint8Array;
	readonly stdout: Uint8Array;
}) => {
	const handle = makeHandle({
		all: Stream.empty,
		exitCode: Effect.succeed(ExitCode(exit_code)),
		getInputFd: () => Sink.drain,
		getOutputFd: () => Stream.empty,
		isRunning: Effect.succeed(false),
		kill: () => Effect.void,
		pid: ProcessId(514),
		stderr: Stream.make(stderr),
		stdin: Sink.drain,
		stdout: Stream.make(stdout),
		unref: Effect.succeed(Effect.void),
	});
	const spawner = make_spawner(() =>
		Effect.acquireRelease(Effect.succeed(handle), () => Effect.sync(() => on_finalize?.())),
	);

	return make_native_directory_picker_layer("win32").pipe(
		Layer.provide(Layer.succeed(ChildProcessSpawner, spawner)),
	);
};

describe("NativeDirectoryPicker", () => {
	it("uses one fixed, shell-free UTF-16LE PowerShell command", () => {
		const command = MakeWindowsNativeDirectoryPickerCommand();

		expect(command.command).toBe("powershell.exe");
		expect(command.options.shell).toBe(false);
		expect(command.options.stdin).toBe("ignore");
		expect(command.options.forceKillAfter).toBe("1 second");
		expect(command.args.slice(0, -1)).toEqual([
			"-NoLogo",
			"-NoProfile",
			"-NonInteractive",
			"-STA",
			"-WindowStyle",
			"Hidden",
			"-EncodedCommand",
		]);
		const script = Buffer.from(command.args.at(-1) ?? "", "base64").toString("utf16le");
		expect(script).toContain("IFileDialog");
		expect(script).toContain("PickFolders");
		expect(script).not.toContain("FolderBrowserDialog");
		expect(script).not.toContain("AutoUpgradeEnabled");
	});

	it.runIf(process.platform === "win32")(
		"compiles and instantiates the Windows native common-item dialog",
		() => {
			const command = MakeWindowsNativeDirectoryPickerProbeCommand();
			const result = spawnSync(command.command, [...command.args], {
				encoding: "utf8",
				shell: false,
				timeout: 15_000,
				windowsHide: true,
			});

			expect({
				error: result.error,
				status: result.status,
				stderr: result.stderr,
			}).toEqual({ error: undefined, status: 0, stderr: "" });
			expect(JSON.parse(result.stdout)).toEqual({ kind: "ready" });
		},
	);

	it("decodes only the selected and cancelled picker protocol", async () => {
		await expect(
			Effect.runPromise(
				DecodeNativeDirectoryPickerResult(
					new TextEncoder().encode('{"kind":"selected","path":"C:\\\\work"}'),
				),
			),
		).resolves.toEqual({ kind: "selected", path: "C:\\work" });
		await expect(
			Effect.runPromise(
				DecodeNativeDirectoryPickerResult(new TextEncoder().encode('{"kind":"cancelled"}')),
			),
		).resolves.toEqual({ kind: "cancelled" });
		const exit = await Effect.runPromiseExit(
			DecodeNativeDirectoryPickerResult(new TextEncoder().encode('{"kind":"selected"}')),
		);
		expect(Exit.isFailure(exit)).toBe(true);
		const multiple = await Effect.runPromiseExit(
			DecodeNativeDirectoryPickerResult(
				new TextEncoder().encode('{"kind":"cancelled"}{"kind":"cancelled"}'),
			),
		);
		expect(Exit.isFailure(multiple)).toBe(true);
	});

	it("accepts a selected directory only from a successful child process", async () => {
		let finalized = false;
		const result = await Effect.runPromise(
			Pick.pipe(
				Effect.provide(
					MakeProcessLayer({
						on_finalize: () => void (finalized = true),
						stdout: new TextEncoder().encode('{"kind":"selected","path":"C:\\\\work"}'),
					}),
				),
			),
		);

		expect(result).toEqual({ kind: "selected", path: "C:\\work" });
		expect(finalized).toBe(true);
	});

	it("treats nonzero exit and oversized output as infrastructure failures", async () => {
		const failed_process = await Effect.runPromise(
			Pick.pipe(
				Effect.flip,
				Effect.provide(
					MakeProcessLayer({
						exit_code: 1,
						stdout: new TextEncoder().encode('{"kind":"cancelled"}'),
					}),
				),
			),
		);
		const oversized_output = await Effect.runPromise(
			Pick.pipe(
				Effect.flip,
				Effect.provide(MakeProcessLayer({ stdout: new Uint8Array(128 * 1024 + 1) })),
			),
		);
		const oversized_error_output = await Effect.runPromise(
			Pick.pipe(
				Effect.flip,
				Effect.provide(
					MakeProcessLayer({
						stderr: new Uint8Array(64 * 1024 + 1),
						stdout: new TextEncoder().encode('{"kind":"cancelled"}'),
					}),
				),
			),
		);

		expect(failed_process.code).toBe("process_failed");
		expect(oversized_output.code).toBe("invalid_output");
		expect(oversized_error_output.code).toBe("invalid_output");
	});

	it("reports unavailable on unsupported platforms", async () => {
		const error = await Effect.runPromise(
			Pick.pipe(Effect.flip, Effect.provide(NativeDirectoryPickerUnavailable)),
		);

		expect(error).toBeInstanceOf(NativeDirectoryPickerError);
		expect(error.code).toBe("unavailable");
	});

	it("fails concurrent selection immediately instead of queuing another dialog", async () => {
		let finalized = false;
		const handle = makeHandle({
			all: Stream.never,
			exitCode: Effect.never,
			getInputFd: () => Sink.drain,
			getOutputFd: () => Stream.empty,
			isRunning: Effect.succeed(true),
			kill: () => Effect.void,
			pid: ProcessId(513),
			stderr: Stream.never,
			stdin: Sink.drain,
			stdout: Stream.never,
			unref: Effect.succeed(Effect.void),
		});
		const spawner = make_spawner(() =>
			Effect.acquireRelease(Effect.succeed(handle), () =>
				Effect.sync(() => void (finalized = true)),
			),
		);
		const layer = make_native_directory_picker_layer("win32").pipe(
			Layer.provide(Layer.succeed(ChildProcessSpawner, spawner)),
		);
		const error = await Effect.runPromise(
			Effect.gen(function* () {
				const first = yield* Pick.pipe(Effect.forkChild({ startImmediately: true }));
				yield* Effect.yieldNow;
				const second = yield* Pick.pipe(Effect.flip);
				yield* Fiber.interrupt(first);
				return second;
			}).pipe(Effect.provide(layer)),
		);

		expect(error.code).toBe("busy");
		expect(finalized).toBe(true);
	});

	it("times out and releases a silent dialog process", async () => {
		let finalized = false;
		const handle = makeHandle({
			all: Stream.never,
			exitCode: Effect.never,
			getInputFd: () => Sink.drain,
			getOutputFd: () => Stream.empty,
			isRunning: Effect.succeed(true),
			kill: () => Effect.void,
			pid: ProcessId(515),
			stderr: Stream.never,
			stdin: Sink.drain,
			stdout: Stream.never,
			unref: Effect.succeed(Effect.void),
		});
		const spawner = make_spawner(() =>
			Effect.acquireRelease(Effect.succeed(handle), () =>
				Effect.sync(() => void (finalized = true)),
			),
		);
		const layer = make_native_directory_picker_layer("win32").pipe(
			Layer.provide(Layer.succeed(ChildProcessSpawner, spawner)),
		);
		const error = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const test_clock = yield* TestClock.make();
					const fiber = yield* Pick.pipe(
						Effect.flip,
						Effect.provideService(Clock.Clock, test_clock),
						Effect.forkChild({ startImmediately: true }),
					);
					yield* Effect.yieldNow;
					yield* test_clock.adjust("5 minutes");
					return yield* Fiber.join(fiber);
				}),
			).pipe(Effect.provide(layer)),
		);

		expect(error.code).toBe("timeout");
		expect(finalized).toBe(true);
	});
});

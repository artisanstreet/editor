import { describe, expect, it } from "vitest";
import { Effect, Layer, Option, Ref } from "effect";

import {
	EncodeWindowsAutostart,
	EncodeWindowsShortcut,
	make_node_windows_native_host_layer,
	ResolveWindowsRegistryLocations,
	WindowsIntegrationAdapterLive,
	WindowsNativeHost,
	type WindowsCommandResult,
} from "../../modules/distribution/src/windows-native-adapter";
import { WindowsIntegrationAdapter } from "../../modules/distribution/src/windows-integrations";

interface RecordedCommand {
	readonly arguments_: ReadonlyArray<string>;
	readonly environment: Readonly<Record<string, string>>;
	readonly executable: string;
}

const Result = (stdout = "", exit_code = 0, stderr = ""): WindowsCommandResult => ({
	exit_code,
	stderr,
	stdout,
});

const MakeAdapter = (
	run: (
		executable: string,
		arguments_: ReadonlyArray<string>,
		environment: Readonly<Record<string, string>>,
	) => WindowsCommandResult,
) =>
	Effect.gen(function* () {
		const calls = yield* Ref.make<ReadonlyArray<RecordedCommand>>([]);
		const host = WindowsNativeHost.of({
			Run: (executable, arguments_, environment = {}) =>
				Ref.update(calls, (current) => [
					...current,
					{ arguments_, environment, executable },
				]).pipe(Effect.as(run(executable, arguments_, environment))),
		});
		const adapter = yield* WindowsIntegrationAdapter.pipe(
			Effect.provide(WindowsIntegrationAdapterLive),
			Effect.provide(Layer.succeed(WindowsNativeHost, host)),
		);
		return { adapter, calls };
	});

describe("WindowsIntegrationAdapterLive", () => {
	it("keeps production registry locations unless an acceptance namespace is explicit", () => {
		expect(ResolveWindowsRegistryLocations({})).toEqual({
			classes: "HKCU\\Software\\Classes\\artisan",
			environment: "HKCU\\Environment",
			protocol_command: "HKCU\\Software\\Classes\\artisan\\shell\\open\\command",
		});
		expect(
			ResolveWindowsRegistryLocations({
				ARTISAN_ACCEPTANCE_REGISTRY_NAMESPACE: "gate_123",
			}),
		).toEqual({
			classes: "HKCU\\Software\\ArtisanAcceptance\\gate_123\\Classes\\artisan",
			environment: "HKCU\\Software\\ArtisanAcceptance\\gate_123\\Environment",
			protocol_command:
				"HKCU\\Software\\ArtisanAcceptance\\gate_123\\Classes\\artisan\\shell\\open\\command",
		});
		expect(() =>
			ResolveWindowsRegistryLocations({
				ARTISAN_ACCEPTANCE_REGISTRY_NAMESPACE: "..\\Environment",
			}),
		).toThrow();
	});

	it("preserves unrelated user PATH entries and broadcasts only after a change", async () => {
		const harness = await Effect.runPromise(
			MakeAdapter((executable, arguments_) => {
				if (executable === "reg.exe" && arguments_[0] === "query")
					return Result("    Path    REG_EXPAND_SZ    C:\\Foreign;C:\\Tools");
				return Result();
			}),
		);

		await Effect.runPromise(
			harness.adapter.WritePathEntry("C:\\Artisan\\bin", "PATH:C:\\Artisan\\bin"),
		);
		await Effect.runPromise(harness.adapter.RemovePathEntry("C:\\Tools"));
		const calls = await Effect.runPromise(Ref.get(harness.calls));
		const writes = calls.filter(
			(call) => call.executable === "reg.exe" && call.arguments_[0] === "add",
		);

		expect(writes[0]?.arguments_).toContain("C:\\Foreign;C:\\Tools;C:\\Artisan\\bin");
		expect(writes[1]?.arguments_).toContain("C:\\Foreign");
		expect(
			calls.filter(
				(call) =>
					call.executable === "powershell.exe" &&
					call.arguments_.includes("-EncodedCommand"),
			),
		).toHaveLength(2);
	});

	it("reads PATH case-insensitively without rewriting it", async () => {
		const harness = await Effect.runPromise(
			MakeAdapter((executable, arguments_) =>
				executable === "reg.exe" && arguments_[0] === "query"
					? Result("Path REG_EXPAND_SZ C:\\ARTISAN\\BIN\\;C:\\Foreign")
					: Result(),
			),
		);

		const content = await Effect.runPromise(harness.adapter.ReadPathEntry("c:\\artisan\\bin"));

		expect(Option.getOrThrow(content)).toBe("PATH:c:\\artisan\\bin");
		expect(await Effect.runPromise(Ref.get(harness.calls))).toHaveLength(1);
	});

	it("writes the HKCU protocol command with one quoted URL argument", async () => {
		const harness = await Effect.runPromise(MakeAdapter(() => Result()));
		const launcher = "C:\\Users\\Sander\\AppData\\Local\\Artisan\\bin\\ae.exe";

		await Effect.runPromise(harness.adapter.WriteProtocol(launcher, `"${launcher}" "%1"`));
		const calls = await Effect.runPromise(Ref.get(harness.calls));

		expect(calls).toHaveLength(3);
		expect(calls[0]?.arguments_).toEqual([
			"add",
			"HKCU\\Software\\Classes\\artisan",
			"/ve",
			"/t",
			"REG_SZ",
			"/d",
			"URL:Artisan Protocol",
			"/f",
		]);
		expect(calls[2]?.arguments_).toContain(`"${launcher}" "%1"`);
		await expect(
			Effect.runPromise(harness.adapter.WriteProtocol(launcher, `${launcher} %1`)),
		).rejects.toMatchObject({ _tag: "WindowsIntegrationAdapterError" });
	});

	it("passes shortcut data through environment variables rather than script interpolation", async () => {
		const shortcut = EncodeWindowsShortcut({
			arguments: "",
			description: "Artisan Editor",
			target_path: "C:\\Artisan\\Artisan Editor.exe",
			working_directory: "C:\\Artisan",
		});
		const harness = await Effect.runPromise(
			MakeAdapter((executable, arguments_) =>
				executable === "powershell.exe" && arguments_.includes("-EncodedCommand")
					? Result(shortcut)
					: Result(),
			),
		);
		const shortcut_path = "C:\\Users\\Sander\\Start Menu\\Artisan Editor.lnk";

		await Effect.runPromise(harness.adapter.WriteShortcut(shortcut_path, shortcut));
		const content = await Effect.runPromise(harness.adapter.ReadShortcut(shortcut_path));
		const calls = await Effect.runPromise(Ref.get(harness.calls));

		expect(Option.getOrThrow(content)).toBe(shortcut);
		expect(calls[0]?.environment).toEqual({
			ARTISAN_CONTENT: shortcut,
			ARTISAN_SHORTCUT: shortcut_path,
		});
		expect(calls[0]?.arguments_.join(" ")).not.toContain(shortcut_path);
	});

	it("uses a current-user scheduled task and idempotent removal", async () => {
		const autostart = EncodeWindowsAutostart({
			arguments: "start",
			executable_path: "C:\\Artisan\\bin\\ae.exe",
			task_name: "Artisan Forge",
		});
		const harness = await Effect.runPromise(
			MakeAdapter((executable, arguments_) =>
				executable === "powershell.exe" && arguments_.includes("-EncodedCommand")
					? Result(autostart)
					: executable === "schtasks.exe"
						? Result("", 1)
						: Result(),
			),
		);

		await Effect.runPromise(harness.adapter.WriteAutostart("Artisan Forge", autostart));
		await Effect.runPromise(harness.adapter.RemoveAutostart("Artisan Forge"));
		const calls = await Effect.runPromise(Ref.get(harness.calls));

		expect(calls[0]?.environment.ARTISAN_TASK).toBe("Artisan Forge");
		expect(calls[0]?.arguments_.join(" ")).not.toContain("Artisan Forge");
		expect(calls[1]).toEqual({
			arguments_: ["/Delete", "/TN", "Artisan Forge", "/F"],
			environment: {},
			executable: "schtasks.exe",
		});
	});
});

describe("NodeWindowsNativeHostLive", () => {
	const RunWithBounds = (
		script: string,
		configuration: {
			readonly max_stderr_bytes: number;
			readonly max_stdout_bytes: number;
			readonly timeout_ms: number;
		},
	) =>
		WindowsNativeHost.pipe(
			Effect.flatMap((host) => host.Run(process.execPath, ["-e", script])),
			Effect.provide(make_node_windows_native_host_layer(configuration)),
			Effect.runPromise,
		);

	it("terminates a command that exceeds its deadline", async () => {
		await expect(
			RunWithBounds("setInterval(() => {}, 1_000)", {
				max_stderr_bytes: 1024,
				max_stdout_bytes: 1024,
				timeout_ms: 50,
			}),
		).rejects.toMatchObject({
			_tag: "WindowsNativeHostError",
			code: "timeout",
			executable: process.execPath,
		});
	});

	it.runIf(process.platform === "win32")(
		"terminates the complete Windows process tree on timeout",
		async () => {
			const root = await mkdtemp(join(tmpdir(), "artisan-native-host-"));
			const marker = join(root, "orphaned-child");
			const child_script = `setTimeout(() => require("node:fs").writeFileSync(${JSON.stringify(marker)}, "alive"), 400)`;
			const parent_script = [
				`require("node:child_process").spawn(process.execPath, ["-e", ${JSON.stringify(child_script)}], { stdio: "ignore" })`,
				"setInterval(() => {}, 1_000)",
			].join(";");
			try {
				await expect(
					RunWithBounds(parent_script, {
						max_stderr_bytes: 1024,
						max_stdout_bytes: 1024,
						timeout_ms: 50,
					}),
				).rejects.toMatchObject({ code: "timeout" });
				await new Promise((resolve) => setTimeout(resolve, 600));
				await expect(access(marker)).rejects.toBeDefined();
			} finally {
				await rm(root, { force: true, recursive: true });
			}
		},
	);

	it.each(["stdout", "stderr"] as const)(
		"terminates a command that overflows %s",
		async (stream) => {
			await expect(
				RunWithBounds(`process.${stream}.write("x".repeat(4096))`, {
					max_stderr_bytes: 64,
					max_stdout_bytes: 64,
					timeout_ms: 5_000,
				}),
			).rejects.toMatchObject({
				_tag: "WindowsNativeHostError",
				code: "output_limit",
				executable: process.execPath,
				stream,
			});
		},
	);
});
import { access, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

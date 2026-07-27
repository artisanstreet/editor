import { spawn } from "node:child_process";

import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import { WindowsIntegrationAdapter, WindowsIntegrationAdapterError } from "./windows-integrations";

export const WindowsShortcut = Schema.Struct({
	arguments: Schema.String,
	description: Schema.optional(Schema.String),
	icon_path: Schema.optional(Schema.String),
	target_path: Schema.NonEmptyString,
	working_directory: Schema.optional(Schema.String),
});
export type WindowsShortcut = typeof WindowsShortcut.Type;

export const WindowsAutostart = Schema.Struct({
	arguments: Schema.String,
	executable_path: Schema.NonEmptyString,
	task_name: Schema.NonEmptyString,
	working_directory: Schema.optional(Schema.String),
});
export type WindowsAutostart = typeof WindowsAutostart.Type;

export const EncodeWindowsShortcut = Schema.encodeSync(Schema.fromJsonString(WindowsShortcut));
export const EncodeWindowsAutostart = Schema.encodeSync(Schema.fromJsonString(WindowsAutostart));

export interface WindowsCommandResult {
	readonly exit_code: number;
	readonly stderr: string;
	readonly stdout: string;
}

export class WindowsNativeHostError extends Data.TaggedError("WindowsNativeHostError")<{
	readonly cause?: unknown;
	readonly code: "output_limit" | "spawn" | "timeout";
	readonly executable: string;
	readonly stream?: "stderr" | "stdout";
}> {}

/** The only native process edge. Commands are always invoked directly with an argv array. */
export class WindowsNativeHost extends Context.Service<
	WindowsNativeHost,
	{
		readonly Run: (
			executable: string,
			arguments_: ReadonlyArray<string>,
			environment?: Readonly<Record<string, string>>,
		) => Effect.Effect<WindowsCommandResult, WindowsNativeHostError>;
	}
>()("Artisan/Distribution/WindowsNativeHost") {}

const DecodeShortcut = Schema.decodeUnknownEffect(Schema.fromJsonString(WindowsShortcut));
const DecodeAutostart = Schema.decodeUnknownEffect(Schema.fromJsonString(WindowsAutostart));

const AcceptanceRegistryNamespace = Schema.NonEmptyString.check(
	Schema.isPattern(/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/),
);

export const ResolveWindowsRegistryLocations = (environment: NodeJS.ProcessEnv = process.env) => {
	const namespace = environment.ARTISAN_ACCEPTANCE_REGISTRY_NAMESPACE;
	if (namespace === undefined)
		return {
			classes: "HKCU\\Software\\Classes\\artisan",
			environment: "HKCU\\Environment",
			protocol_command: "HKCU\\Software\\Classes\\artisan\\shell\\open\\command",
		} as const;
	const validated = Schema.decodeUnknownSync(AcceptanceRegistryNamespace)(namespace);
	const root = `HKCU\\Software\\ArtisanAcceptance\\${validated}`;
	const classes = `${root}\\Classes\\artisan`;
	return {
		classes,
		environment: `${root}\\Environment`,
		protocol_command: `${classes}\\shell\\open\\command`,
	} as const;
};

const powershell_read_shortcut = [
	"if (-not (Test-Path -LiteralPath $env:ARTISAN_SHORTCUT -PathType Leaf)) { exit 3 }",
	"$shell = New-Object -ComObject WScript.Shell",
	"$shortcut = $shell.CreateShortcut($env:ARTISAN_SHORTCUT)",
	"$result = [ordered]@{ arguments = $shortcut.Arguments; target_path = $shortcut.TargetPath }",
	"if ($shortcut.Description) { $result.description = $shortcut.Description }",
	"if ($shortcut.IconLocation) { $result.icon_path = ($shortcut.IconLocation -replace ',0$','') }",
	"if ($shortcut.WorkingDirectory) { $result.working_directory = $shortcut.WorkingDirectory }",
	"$result | ConvertTo-Json -Compress",
].join("\n");

const powershell_write_shortcut = [
	"$value = $env:ARTISAN_CONTENT | ConvertFrom-Json",
	"$parent = Split-Path -Parent $env:ARTISAN_SHORTCUT",
	"[System.IO.Directory]::CreateDirectory($parent) | Out-Null",
	"$shell = New-Object -ComObject WScript.Shell",
	"$shortcut = $shell.CreateShortcut($env:ARTISAN_SHORTCUT)",
	"$shortcut.TargetPath = $value.target_path",
	"$shortcut.Arguments = $value.arguments",
	"if ($value.description) { $shortcut.Description = $value.description }",
	"if ($value.icon_path) { $shortcut.IconLocation = $value.icon_path }",
	"if ($value.working_directory) { $shortcut.WorkingDirectory = $value.working_directory }",
	"$shortcut.Save()",
].join("\n");

const powershell_read_task = [
	"$task = Get-ScheduledTask -TaskName $env:ARTISAN_TASK -ErrorAction Stop",
	"$action = @($task.Actions)[0]",
	"$result = [ordered]@{ arguments = $action.Arguments; executable_path = $action.Execute; task_name = $task.TaskName }",
	"if ($action.WorkingDirectory) { $result.working_directory = $action.WorkingDirectory }",
	"$result | ConvertTo-Json -Compress",
].join("\n");

const powershell_write_task = [
	"$value = $env:ARTISAN_CONTENT | ConvertFrom-Json",
	"$action_options = @{ Execute = $value.executable_path; Argument = $value.arguments }",
	"if ($value.working_directory) { $action_options.WorkingDirectory = $value.working_directory }",
	"$action = New-ScheduledTaskAction @action_options",
	"$trigger = New-ScheduledTaskTrigger -AtLogOn",
	"$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited",
	"Register-ScheduledTask -TaskName $value.task_name -Action $action -Trigger $trigger -Principal $principal -Force | Out-Null",
].join("\n");

const powershell_broadcast_environment = [
	"$signature = '[DllImport(\"user32.dll\", SetLastError=true, CharSet=CharSet.Auto)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint flags, uint timeout, out UIntPtr result);'",
	"$native = Add-Type -MemberDefinition $signature -Name NativeMethods -Namespace Artisan -PassThru",
	"$result = [UIntPtr]::Zero",
	"[void]$native::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, 'Environment', 0x0002, 5000, [ref]$result)",
].join("\n");

const EncodedPowerShell = (script: string) => Buffer.from(script, "utf16le").toString("base64");

const RunPowerShell = (
	host: WindowsNativeHost["Service"],
	script: string,
	environment: Readonly<Record<string, string>>,
) =>
	host.Run(
		"powershell.exe",
		["-NoLogo", "-NoProfile", "-NonInteractive", "-EncodedCommand", EncodedPowerShell(script)],
		environment,
	);

const AdapterError = (operation: WindowsIntegrationAdapterError["operation"]) => (cause: unknown) =>
	new WindowsIntegrationAdapterError({ cause, operation });

const RequireSuccess = (
	result: WindowsCommandResult,
	operation: WindowsIntegrationAdapterError["operation"],
) =>
	result.exit_code === 0
		? Effect.void
		: Effect.fail(
				new WindowsIntegrationAdapterError({
					cause: result.stderr,
					operation,
				}),
			);

const QueryRegistryValue = (
	host: WindowsNativeHost["Service"],
	key: string,
	value_name: string | undefined,
) =>
	host
		.Run("reg.exe", [
			"query",
			key,
			...(value_name === undefined ? ["/ve"] : ["/v", value_name]),
		])
		.pipe(
			Effect.map((result) => {
				if (result.exit_code !== 0) return Option.none<string>();
				const match = result.stdout.match(/REG_(?:SZ|EXPAND_SZ)\s+([^\r\n]*)/u);
				const value = match?.[1];
				return value === undefined ? Option.none<string>() : Option.some(value.trim());
			}),
		);

const SetRegistryValue = (
	host: WindowsNativeHost["Service"],
	key: string,
	value_name: string | undefined,
	value: string,
	value_type = "REG_SZ",
) =>
	host
		.Run("reg.exe", [
			"add",
			key,
			...(value_name === undefined ? ["/ve"] : ["/v", value_name]),
			"/t",
			value_type,
			"/d",
			value,
			"/f",
		])
		.pipe(Effect.flatMap((result) => RequireSuccess(result, "write")));

const NormalizePathEntry = (value: string) =>
	value
		.trim()
		.replace(/^"(.*)"$/u, "$1")
		.replace(/[\\/]+$/u, "")
		.toLocaleLowerCase("en-US");

const ParsePath = (value: string) =>
	value
		.split(";")
		.map((entry) => entry.trim())
		.filter((entry) => entry.length > 0);

const ReadUserPath = (host: WindowsNativeHost["Service"]) =>
	QueryRegistryValue(host, ResolveWindowsRegistryLocations().environment, "Path").pipe(
		Effect.map(Option.getOrElse(() => "")),
	);

const WriteUserPath = (host: WindowsNativeHost["Service"], entries: ReadonlyArray<string>) =>
	SetRegistryValue(
		host,
		ResolveWindowsRegistryLocations().environment,
		"Path",
		entries.join(";"),
		"REG_EXPAND_SZ",
	).pipe(
		Effect.tap(() =>
			RunPowerShell(host, powershell_broadcast_environment, {}).pipe(Effect.ignore),
		),
	);

const CanonicalPathContent = (target_path: string) => `PATH:${target_path}`;

export const WindowsIntegrationAdapterLive = Layer.effect(
	WindowsIntegrationAdapter,
	Effect.gen(function* () {
		const host = yield* WindowsNativeHost;

		return WindowsIntegrationAdapter.of({
			ReadPathEntry: (target_path) =>
				ReadUserPath(host).pipe(
					Effect.map((value) =>
						ParsePath(value).some(
							(entry) =>
								NormalizePathEntry(entry) === NormalizePathEntry(target_path),
						)
							? Option.some(CanonicalPathContent(target_path))
							: Option.none(),
					),
					Effect.mapError(AdapterError("read")),
				),
			WritePathEntry: (target_path, content) =>
				Effect.gen(function* () {
					if (content !== CanonicalPathContent(target_path))
						return yield* Effect.fail(
							new WindowsIntegrationAdapterError({
								cause: "PATH content is not canonical",
								operation: "write",
							}),
						);
					const entries = ParsePath(yield* ReadUserPath(host));
					if (
						entries.some(
							(entry) =>
								NormalizePathEntry(entry) === NormalizePathEntry(target_path),
						)
					)
						return;
					yield* WriteUserPath(host, [...entries, target_path]);
				}).pipe(Effect.mapError(AdapterError("write"))),
			RemovePathEntry: (target_path) =>
				Effect.gen(function* () {
					const entries = ParsePath(yield* ReadUserPath(host));
					const retained = entries.filter(
						(entry) => NormalizePathEntry(entry) !== NormalizePathEntry(target_path),
					);
					if (retained.length !== entries.length) yield* WriteUserPath(host, retained);
				}).pipe(Effect.mapError(AdapterError("remove"))),
			ReadProtocol: () =>
				QueryRegistryValue(
					host,
					ResolveWindowsRegistryLocations().protocol_command,
					undefined,
				).pipe(Effect.mapError(AdapterError("read"))),
			WriteProtocol: (target_path, content) =>
				Effect.gen(function* () {
					const registry = ResolveWindowsRegistryLocations();
					const expected = `"${target_path}" "%1"`;
					if (content !== expected)
						return yield* Effect.fail(
							new WindowsIntegrationAdapterError({
								cause: `protocol command must be ${expected}`,
								operation: "write",
							}),
						);
					yield* SetRegistryValue(
						host,
						registry.classes,
						undefined,
						"URL:Artisan Protocol",
					);
					yield* SetRegistryValue(host, registry.classes, "URL Protocol", "");
					yield* SetRegistryValue(host, registry.protocol_command, undefined, expected);
				}).pipe(Effect.mapError(AdapterError("write"))),
			RemoveProtocol: () =>
				host
					.Run("reg.exe", ["delete", ResolveWindowsRegistryLocations().classes, "/f"])
					.pipe(
						Effect.flatMap((result) =>
							result.exit_code === 0 || result.exit_code === 1
								? Effect.void
								: RequireSuccess(result, "remove"),
						),
						Effect.mapError(AdapterError("remove")),
					),
			ReadShortcut: (shortcut_path) =>
				RunPowerShell(host, powershell_read_shortcut, {
					ARTISAN_SHORTCUT: shortcut_path,
				}).pipe(
					Effect.flatMap((result) =>
						result.exit_code === 0
							? DecodeShortcut(result.stdout.trim()).pipe(
									Effect.map((value) =>
										Option.some(EncodeWindowsShortcut(value)),
									),
								)
							: Effect.succeed(Option.none()),
					),
					Effect.mapError(AdapterError("read")),
				),
			WriteShortcut: (shortcut_path, content) =>
				DecodeShortcut(content).pipe(
					Effect.flatMap((value) =>
						RunPowerShell(host, powershell_write_shortcut, {
							ARTISAN_CONTENT: EncodeWindowsShortcut(value),
							ARTISAN_SHORTCUT: shortcut_path,
						}),
					),
					Effect.flatMap((result) => RequireSuccess(result, "write")),
					Effect.mapError(AdapterError("write")),
				),
			RemoveShortcut: (shortcut_path) =>
				host
					.Run(
						"powershell.exe",
						[
							"-NoLogo",
							"-NoProfile",
							"-NonInteractive",
							"-Command",
							"Remove-Item -LiteralPath $env:ARTISAN_SHORTCUT -Force -ErrorAction SilentlyContinue",
						],
						{ ARTISAN_SHORTCUT: shortcut_path },
					)
					.pipe(
						Effect.flatMap((result) => RequireSuccess(result, "remove")),
						Effect.mapError(AdapterError("remove")),
					),
			ReadAutostart: (target_path) =>
				RunPowerShell(host, powershell_read_task, {
					ARTISAN_TASK: target_path,
				}).pipe(
					Effect.flatMap((result) =>
						result.exit_code === 0
							? DecodeAutostart(result.stdout.trim()).pipe(
									Effect.map((value) =>
										Option.some(EncodeWindowsAutostart(value)),
									),
								)
							: Effect.succeed(Option.none()),
					),
					Effect.mapError(AdapterError("read")),
				),
			WriteAutostart: (target_path, content) =>
				DecodeAutostart(content).pipe(
					Effect.filterOrFail(
						(value) => value.task_name === target_path,
						() => "task name does not match integration path",
					),
					Effect.flatMap((value) =>
						RunPowerShell(host, powershell_write_task, {
							ARTISAN_CONTENT: EncodeWindowsAutostart(value),
							ARTISAN_TASK: target_path,
						}),
					),
					Effect.flatMap((result) => RequireSuccess(result, "write")),
					Effect.mapError(AdapterError("write")),
				),
			RemoveAutostart: (target_path) =>
				host.Run("schtasks.exe", ["/Delete", "/TN", target_path, "/F"]).pipe(
					Effect.flatMap((result) =>
						result.exit_code === 0 || result.exit_code === 1
							? Effect.void
							: RequireSuccess(result, "remove"),
					),
					Effect.mapError(AdapterError("remove")),
				),
		});
	}),
);

export interface NodeWindowsNativeHostConfiguration {
	readonly max_stderr_bytes: number;
	readonly max_stdout_bytes: number;
	readonly timeout_ms: number;
}

const production_native_host_configuration: NodeWindowsNativeHostConfiguration = {
	max_stderr_bytes: 1024 * 1024,
	max_stdout_bytes: 1024 * 1024,
	timeout_ms: 30_000,
};

const KillProcessTree = (child: ReturnType<typeof spawn>) => {
	if (child.pid !== undefined && process.platform === "win32") {
		const killer = spawn("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], {
			shell: false,
			stdio: "ignore",
			windowsHide: true,
		});
		killer.once("error", () => {
			child.kill();
		});
		killer.unref();
		return;
	}
	child.kill();
};

export const make_node_windows_native_host_layer = (
	configuration: NodeWindowsNativeHostConfiguration = production_native_host_configuration,
) =>
	Layer.succeed(WindowsNativeHost, {
		Run: (executable, arguments_, environment = {}) =>
			Effect.callback<WindowsCommandResult, WindowsNativeHostError>((resume) => {
				const child = spawn(executable, [...arguments_], {
					env: { ...process.env, ...environment },
					shell: false,
					windowsHide: true,
				});
				const stdout_chunks: Array<Buffer> = [];
				const stderr_chunks: Array<Buffer> = [];
				let stdout_bytes = 0;
				let stderr_bytes = 0;
				let settled = false;
				const timer = setTimeout(() => {
					if (settled) return;
					settled = true;
					KillProcessTree(child);
					resume(
						Effect.fail(
							new WindowsNativeHostError({
								code: "timeout",
								executable,
							}),
						),
					);
				}, configuration.timeout_ms);
				timer.unref();
				const FailOutputLimit = (stream: "stderr" | "stdout") => {
					if (settled) return;
					settled = true;
					clearTimeout(timer);
					KillProcessTree(child);
					resume(
						Effect.fail(
							new WindowsNativeHostError({
								code: "output_limit",
								executable,
								stream,
							}),
						),
					);
				};
				child.stdout.on("data", (value: Buffer) => {
					stdout_bytes += value.byteLength;
					if (stdout_bytes > configuration.max_stdout_bytes)
						return FailOutputLimit("stdout");
					stdout_chunks.push(value);
				});
				child.stderr.on("data", (value: Buffer) => {
					stderr_bytes += value.byteLength;
					if (stderr_bytes > configuration.max_stderr_bytes)
						return FailOutputLimit("stderr");
					stderr_chunks.push(value);
				});
				child.once("error", (cause) => {
					if (settled) return;
					settled = true;
					clearTimeout(timer);
					resume(
						Effect.fail(
							new WindowsNativeHostError({ cause, code: "spawn", executable }),
						),
					);
				});
				child.once("close", (exit_code) => {
					if (settled) return;
					settled = true;
					clearTimeout(timer);
					resume(
						Effect.succeed({
							exit_code: exit_code ?? -1,
							stderr: Buffer.concat(stderr_chunks, stderr_bytes).toString("utf8"),
							stdout: Buffer.concat(stdout_chunks, stdout_bytes).toString("utf8"),
						}),
					);
				});
				return Effect.sync(() => {
					if (settled) return;
					settled = true;
					clearTimeout(timer);
					KillProcessTree(child);
				});
			}),
	});

export const NodeWindowsNativeHostLive = make_node_windows_native_host_layer();

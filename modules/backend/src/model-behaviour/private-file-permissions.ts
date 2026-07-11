import { Context, Data, Effect, Layer } from "effect";
import { FileSystem } from "effect/FileSystem";
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process";

/** Identifies the host permission model used to restrict a private file. */
export type PrivateFilePermissionsPlatformKind = "posix" | "win32";

/** Reports a failed private-file permission operation. */
export class PrivateFilePermissionsError extends Data.TaggedError("PrivateFilePermissionsError")<{
	readonly cause: unknown;
	readonly operation: "acl" | "chmod";
	readonly path: string;
}> {}

/** Supplies the host platform required by private-file permission policy. */
export class PrivateFilePermissionsPlatform extends Context.Service<
	PrivateFilePermissionsPlatform,
	{
		readonly kind: PrivateFilePermissionsPlatformKind;
	}
>()("Artisan/PrivateFilePermissionsPlatform") {}

/** Restricts private config files to the current operating-system user. */
export class PrivateFilePermissions extends Context.Service<
	PrivateFilePermissions,
	{
		readonly Restrict: (path: string) => Effect.Effect<void, PrivateFilePermissionsError>;
	}
>()("Artisan/PrivateFilePermissions") {}

const powershell_acl_script = [
	"$path = $env:ARTISAN_PRIVATE_FILE_PATH",
	"$current_user = [System.Security.Principal.WindowsIdentity]::GetCurrent().User",
	"$acl = Get-Acl -LiteralPath $path",
	"$acl.SetAccessRuleProtection($true, $false)",
	"foreach ($existing_rule in @($acl.Access)) { [void]$acl.RemoveAccessRuleAll($existing_rule) }",
	'$rule = New-Object System.Security.AccessControl.FileSystemAccessRule($current_user, "FullControl", "Allow")',
	"$acl.SetAccessRule($rule)",
	"Set-Acl -LiteralPath $path -AclObject $acl",
].join("; ");

function permission_error(
	path: string,
	operation: PrivateFilePermissionsError["operation"],
	cause: unknown,
) {
	return new PrivateFilePermissionsError({ cause, operation, path });
}

function make_windows_acl_command(path: string) {
	return ChildProcess.make(
		"powershell.exe",
		[
			"-NoLogo",
			"-NoProfile",
			"-NonInteractive",
			"-ExecutionPolicy",
			"Bypass",
			"-Command",
			powershell_acl_script,
		],
		{
			env: { ARTISAN_PRIVATE_FILE_PATH: path },
			extendEnv: true,
			shell: false,
		},
	);
}

/**
 * Builds the platform-neutral private-file permission policy.
 *
 * @example
 * ```ts
 * const layer = make_private_file_permissions_layer.pipe(
 *   Layer.provideMerge(FileSystem.layer),
 * );
 * ```
 *
 * @since 0.1.0
 * @returns A layer that requires Effect's filesystem, process-spawner, and
 * platform-kind services.
 */
export const make_private_file_permissions_layer = Layer.effect(
	PrivateFilePermissions,
	Effect.gen(function* () {
		const file_system = yield* FileSystem;
		const child_process_spawner = yield* ChildProcessSpawner.ChildProcessSpawner;
		const platform = yield* PrivateFilePermissionsPlatform;

		return {
			Restrict: (path) =>
				platform.kind === "posix"
					? file_system
							.chmod(path, 0o600)
							.pipe(
								Effect.mapError((cause) => permission_error(path, "chmod", cause)),
							)
					: Effect.scoped(
							child_process_spawner.exitCode(make_windows_acl_command(path)),
						).pipe(
							Effect.flatMap((exit_code) =>
								exit_code === 0
									? Effect.void
									: Effect.fail(
											permission_error(
												path,
												"acl",
												new Error(
													`PowerShell exited with code ${exit_code}`,
												),
											),
										),
							),
							Effect.mapError((cause) =>
								cause instanceof PrivateFilePermissionsError
									? cause
									: permission_error(path, "acl", cause),
							),
						),
		};
	}),
);

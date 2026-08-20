import { fchmod } from "node:fs";

import { Context, Data, Effect, Layer, Option, PlatformError, Schema, Stream } from "effect";
import { FileSystem } from "effect/FileSystem";
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process";
import { FileDescriptorOf } from "../filesystem/node/file-descriptor";

import {
	ReadFileIdentity,
	same_file_identity,
	type FileIdentity,
} from "../filesystem/file-identity";

/** Identifies the host permission model used to restrict a private file. */
export type PrivateFilePermissionsPlatformKind = "posix" | "win32";

/** Preserves the private-file identity type exported by the original module. */
export type PrivateFileIdentity = FileIdentity;

/** Preserves the private-file identity reader exported by the original module. */
export const ReadPrivateFileIdentity = ReadFileIdentity;

/** Captures POSIX permission bits before a private file is restricted. */
export class PosixPrivateFilePermissionsSnapshot extends Data.TaggedClass(
	"PosixPrivateFilePermissionsSnapshot",
)<{
	readonly mode: number;
}> {}

/** Captures a Windows file security descriptor as a Security Descriptor Definition Language string. */
export class WindowsPrivateFilePermissionsSnapshot extends Data.TaggedClass(
	"WindowsPrivateFilePermissionsSnapshot",
)<{
	readonly sddl: string;
}> {}

/** Represents the permission state captured before private-file restriction. */
export type PrivateFilePermissionsSnapshot =
	| PosixPrivateFilePermissionsSnapshot
	| WindowsPrivateFilePermissionsSnapshot;

/** Reports a failed private-file permission capture. */
export class PrivateFilePermissionsCaptureError extends Data.TaggedError(
	"PrivateFilePermissionsCaptureError",
)<{
	readonly cause: unknown;
	readonly path: string;
}> {}

/** Reports a failed private-file creation. */
export class PrivateFilePermissionsCreateError extends Data.TaggedError(
	"PrivateFilePermissionsCreateError",
)<{
	readonly cause: unknown;
	readonly path: string;
}> {}

/** Reports a failed private-file permission restriction. */
export class PrivateFilePermissionsRestrictError extends Data.TaggedError(
	"PrivateFilePermissionsRestrictError",
)<{
	readonly cause: unknown;
	readonly path: string;
}> {}

/** Reports a failed private-file permission restoration. */
export class PrivateFilePermissionsRestoreError extends Data.TaggedError(
	"PrivateFilePermissionsRestoreError",
)<{
	readonly cause: unknown;
	readonly path: string;
}> {}

/** Reports an attempt to restore a snapshot for a different host platform. */
export class PrivateFilePermissionsSnapshotPlatformMismatchError extends Data.TaggedError(
	"PrivateFilePermissionsSnapshotPlatformMismatchError",
)<{
	readonly path: string;
	readonly platform: PrivateFilePermissionsPlatformKind;
	readonly snapshot: PrivateFilePermissionsSnapshot;
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
		readonly Capture: (
			path: string,
		) => Effect.Effect<PrivateFilePermissionsSnapshot, PrivateFilePermissionsCaptureError>;
		readonly CreatePrivate: (
			path: string,
		) => Effect.Effect<Option.Option<PrivateFileIdentity>, PrivateFilePermissionsCreateError>;
		readonly Restrict: (
			path: string,
		) => Effect.Effect<void, PrivateFilePermissionsRestrictError>;
		readonly RestrictDirectory: (
			path: string,
		) => Effect.Effect<void, PrivateFilePermissionsRestrictError>;
		readonly RestrictOwned: (
			path: string,
			identity: PrivateFileIdentity,
		) => Effect.Effect<boolean, PrivateFilePermissionsRestrictError>;
		readonly Restore: (
			path: string,
			snapshot: PrivateFilePermissionsSnapshot,
		) => Effect.Effect<
			void,
			PrivateFilePermissionsRestoreError | PrivateFilePermissionsSnapshotPlatformMismatchError
		>;
		readonly RestoreOwned: (
			path: string,
			identity: PrivateFileIdentity,
			snapshot: PrivateFilePermissionsSnapshot,
		) => Effect.Effect<
			boolean,
			PrivateFilePermissionsRestoreError | PrivateFilePermissionsSnapshotPlatformMismatchError
		>;
	}
>()("Artisan/PrivateFilePermissions") {}

const powershell_capture_acl_script = [
	'$ErrorActionPreference = "Stop"',
	"$acl = [System.IO.File]::GetAccessControl($env:ARTISAN_PRIVATE_FILE_PATH)",
	"$sddl = [regex]::Replace($acl.Sddl, \"D:(?<prefix>(?:P|AR)*)AI(?=\\()\", 'D:${prefix}')",
	"[Console]::Out.Write($sddl)",
].join("; ");

const powershell_restrict_acl_script = [
	'$ErrorActionPreference = "Stop"',
	"$path = $env:ARTISAN_PRIVATE_FILE_PATH",
	"$current_user = [System.Security.Principal.WindowsIdentity]::GetCurrent().User",
	"$acl = New-Object System.Security.AccessControl.FileSecurity",
	"$acl.SetAccessRuleProtection($true, $false)",
	'$rule = New-Object System.Security.AccessControl.FileSystemAccessRule($current_user, "FullControl", "Allow")',
	"$acl.SetAccessRule($rule)",
	"[System.IO.File]::SetAccessControl($path, $acl)",
].join("; ");

const powershell_restrict_directory_acl_script = [
	'$ErrorActionPreference = "Stop"',
	"$path = $env:ARTISAN_PRIVATE_FILE_PATH",
	"$current_user = [System.Security.Principal.WindowsIdentity]::GetCurrent().User",
	"$acl = New-Object System.Security.AccessControl.DirectorySecurity",
	"$acl.SetAccessRuleProtection($true, $false)",
	'$rule = New-Object System.Security.AccessControl.FileSystemAccessRule($current_user, "FullControl", "ContainerInherit, ObjectInherit", "None", "Allow")',
	"$acl.SetAccessRule($rule)",
	"[System.IO.Directory]::SetAccessControl($path, $acl)",
].join("; ");

const powershell_restore_acl_script = [
	'$ErrorActionPreference = "Stop"',
	"$path = $env:ARTISAN_PRIVATE_FILE_PATH",
	"$sddl = $env:ARTISAN_PRIVATE_FILE_SDDL",
	"$access = [System.Security.AccessControl.AccessControlSections]::Access",
	"$acl = New-Object System.Security.AccessControl.FileSecurity",
	"$acl.SetSecurityDescriptorSddlForm($sddl, $access)",
	"[System.IO.File]::SetAccessControl($path, $acl)",
].join("; ");

const windows_file_identity_source = `
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

public static class ArtisanFileIdentity {
    [StructLayout(LayoutKind.Sequential)]
    private struct FileTime {
        public uint Low;
        public uint High;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ByHandleFileInformation {
        public uint FileAttributes;
        public FileTime CreationTime;
        public FileTime LastAccessTime;
        public FileTime LastWriteTime;
        public uint VolumeSerialNumber;
        public uint FileSizeHigh;
        public uint FileSizeLow;
        public uint NumberOfLinks;
        public uint FileIndexHigh;
        public uint FileIndexLow;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandle(
        SafeFileHandle handle,
        out ByHandleFileInformation information
    );

    public static string Identity(FileStream stream) {
        ByHandleFileInformation information;
        if (!GetFileInformationByHandle(stream.SafeFileHandle, out information)) {
            throw new Win32Exception(Marshal.GetLastWin32Error());
        }

        ulong inode = ((ulong)information.FileIndexHigh << 32) | information.FileIndexLow;
        return information.VolumeSerialNumber.ToString(System.Globalization.CultureInfo.InvariantCulture)
            + ":"
            + inode.ToString(System.Globalization.CultureInfo.InvariantCulture);
    }
}
`;

const powershell_owned_stream = [
	"$rights = [System.Security.AccessControl.FileSystemRights]::ReadData -bor [System.Security.AccessControl.FileSystemRights]::WriteData -bor [System.Security.AccessControl.FileSystemRights]::ReadPermissions -bor [System.Security.AccessControl.FileSystemRights]::ChangePermissions",
	"$stream = [System.IO.FileStream]::new($env:ARTISAN_PRIVATE_FILE_PATH, [System.IO.FileMode]::Open, $rights, [System.IO.FileShare]::None, 4096, [System.IO.FileOptions]::None)",
].join("; ");

const powershell_create_private_file_script = [
	'$ErrorActionPreference = "Stop"',
	"Add-Type -TypeDefinition $env:ARTISAN_FILE_ID_SOURCE",
	"$rights = [System.Security.AccessControl.FileSystemRights]::ReadData -bor [System.Security.AccessControl.FileSystemRights]::WriteData -bor [System.Security.AccessControl.FileSystemRights]::ReadPermissions -bor [System.Security.AccessControl.FileSystemRights]::ChangePermissions",
	"$current_user = [System.Security.Principal.WindowsIdentity]::GetCurrent().User",
	"$acl = New-Object System.Security.AccessControl.FileSecurity",
	"$acl.SetAccessRuleProtection($true, $false)",
	'$rule = New-Object System.Security.AccessControl.FileSystemAccessRule($current_user, "FullControl", "Allow")',
	"$acl.SetAccessRule($rule)",
	'try { $stream = [System.IO.FileStream]::new($env:ARTISAN_PRIVATE_FILE_PATH, [System.IO.FileMode]::CreateNew, $rights, [System.IO.FileShare]::None, 4096, [System.IO.FileOptions]::None, $acl) } catch [System.IO.IOException] { $code = $_.Exception.HResult -band 0xffff; if ($code -eq 80 -or $code -eq 183) { [Console]::Out.Write("exists"); return }; throw }',
	"try {",
	"$identity = [ArtisanFileIdentity]::Identity($stream)",
	'[Console]::Out.Write("created:" + $identity)',
	"} finally { $stream.Dispose() }",
].join("; ");

const powershell_owned_restrict_acl_script = [
	'$ErrorActionPreference = "Stop"',
	"Add-Type -TypeDefinition $env:ARTISAN_FILE_ID_SOURCE",
	powershell_owned_stream,
	"try {",
	"$identity = [ArtisanFileIdentity]::Identity($stream)",
	'if ($identity -ne $env:ARTISAN_PRIVATE_FILE_IDENTITY) { [Console]::Out.Write("changed"); return }',
	"$current_user = [System.Security.Principal.WindowsIdentity]::GetCurrent().User",
	"$acl = New-Object System.Security.AccessControl.FileSecurity",
	"$acl.SetAccessRuleProtection($true, $false)",
	'$rule = New-Object System.Security.AccessControl.FileSystemAccessRule($current_user, "FullControl", "Allow")',
	"$acl.SetAccessRule($rule)",
	"$stream.SetAccessControl($acl)",
	'[Console]::Out.Write("applied")',
	"} finally { $stream.Dispose() }",
].join("; ");

const powershell_owned_restore_acl_script = [
	'$ErrorActionPreference = "Stop"',
	"Add-Type -TypeDefinition $env:ARTISAN_FILE_ID_SOURCE",
	powershell_owned_stream,
	"try {",
	"$identity = [ArtisanFileIdentity]::Identity($stream)",
	'if ($identity -ne $env:ARTISAN_PRIVATE_FILE_IDENTITY) { [Console]::Out.Write("changed"); return }',
	"$access = [System.Security.AccessControl.AccessControlSections]::Access",
	"$acl = New-Object System.Security.AccessControl.FileSecurity",
	"$acl.SetSecurityDescriptorSddlForm($env:ARTISAN_PRIVATE_FILE_SDDL, $access)",
	"$stream.SetAccessControl($acl)",
	'[Console]::Out.Write("applied")',
	"} finally { $stream.Dispose() }",
].join("; ");

function make_capture_error(path: string, cause: unknown) {
	return new PrivateFilePermissionsCaptureError({ cause, path });
}

function make_create_error(path: string, cause: unknown) {
	return new PrivateFilePermissionsCreateError({ cause, path });
}

function make_restrict_error(path: string, cause: unknown) {
	return new PrivateFilePermissionsRestrictError({ cause, path });
}

function make_restore_error(path: string, cause: unknown) {
	return new PrivateFilePermissionsRestoreError({ cause, path });
}

function make_windows_acl_command(
	path: string,
	script: string,
	options: { readonly identity?: PrivateFileIdentity; readonly sddl?: string } = {},
) {
	return ChildProcess.make(
		"powershell.exe",
		[
			"-NoLogo",
			"-NoProfile",
			"-NonInteractive",
			"-ExecutionPolicy",
			"Bypass",
			"-Command",
			script,
		],
		{
			env: {
				ARTISAN_PRIVATE_FILE_PATH: path,
				ARTISAN_FILE_ID_SOURCE: windows_file_identity_source,
				...(options.identity === undefined
					? {}
					: {
							ARTISAN_PRIVATE_FILE_IDENTITY: `${options.identity.device}:${options.identity.inode}`,
						}),
				...(options.sddl === undefined ? {} : { ARTISAN_PRIVATE_FILE_SDDL: options.sddl }),
			},
			extendEnv: true,
			shell: false,
		},
	);
}

const StoredPrivateFileIdentity = Schema.Struct({
	device: Schema.BigIntFromString,
	inode: Schema.BigIntFromString,
});

function ParseCreatedWindowsIdentity(path: string, output: string) {
	const value = output.trim();

	if (value === "exists") {
		return Effect.succeed(Option.none<PrivateFileIdentity>());
	}

	const fields = value.startsWith("created:") ? value.slice("created:".length).split(":") : [];
	const encoded = fields.length === 2 ? { device: fields[0], inode: fields[1] } : {};

	return Schema.decodeUnknownEffect(StoredPrivateFileIdentity, {
		onExcessProperty: "error",
	})(encoded).pipe(
		Effect.map(Option.some),
		Effect.mapError((cause) => make_create_error(path, cause)),
	);
}

function RunWindowsAclCommand(
	child_process_spawner: ChildProcessSpawner.ChildProcessSpawner["Service"],
	command: ChildProcess.Command,
) {
	return Effect.scoped(
		Effect.gen(function* () {
			const child_process = yield* child_process_spawner.spawn(command);
			const { output, standard_error } = yield* Effect.all(
				{
					output: child_process.stdout.pipe(Stream.decodeText(), Stream.mkString),
					standard_error: child_process.stderr.pipe(Stream.decodeText(), Stream.mkString),
				},
				{ concurrency: "unbounded" },
			);
			const exit_code = yield* child_process.exitCode;

			if (exit_code !== 0) {
				return yield* Effect.fail(
					new Error(`PowerShell exited with code ${exit_code}: ${standard_error.trim()}`),
				);
			}

			return output;
		}),
	);
}

function Fchmod(descriptor: number, mode: number) {
	return Effect.callback<void, Error>((resume) => {
		fchmod(descriptor, mode, (cause) => {
			resume(cause === null ? Effect.void : Effect.fail(cause));
		});
	});
}

function ApplyPosixOwned(
	file_system: FileSystem,
	path: string,
	identity: PrivateFileIdentity,
	mode: number,
) {
	return Effect.scoped(
		Effect.gen(function* () {
			const file = yield* file_system.open(path, { flag: "r+" });
			const descriptor = yield* FileDescriptorOf(file, "private file permissions");
			const current_identity = yield* ReadPrivateFileIdentity(descriptor);

			if (!same_file_identity(current_identity, identity)) {
				return false;
			}

			/** Same descriptor the identity was proven on, so nothing can be swapped between. */
			yield* Fchmod(descriptor, mode);

			return true;
		}),
	);
}

function ApplyWindowsOwned(
	child_process_spawner: ChildProcessSpawner.ChildProcessSpawner["Service"],
	path: string,
	identity: PrivateFileIdentity,
	script: string,
	sddl?: string,
) {
	if (identity.inode === 0n) {
		return Effect.succeed(false);
	}

	return RunWindowsAclCommand(
		child_process_spawner,
		make_windows_acl_command(path, script, {
			identity,
			...(sddl === undefined ? {} : { sddl }),
		}),
	).pipe(Effect.map((result) => result.trim() === "applied"));
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
			Capture: (path) =>
				platform.kind === "posix"
					? file_system.stat(path).pipe(
							Effect.map(
								(info) =>
									new PosixPrivateFilePermissionsSnapshot({
										mode: info.mode & 0o7777,
									}),
							),
							Effect.mapError((cause) => make_capture_error(path, cause)),
						)
					: RunWindowsAclCommand(
							child_process_spawner,
							make_windows_acl_command(path, powershell_capture_acl_script),
						).pipe(
							Effect.flatMap((sddl) =>
								sddl.length > 0
									? Effect.succeed(
											new WindowsPrivateFilePermissionsSnapshot({ sddl }),
										)
									: Effect.fail(
											new Error(
												"PowerShell returned an empty security descriptor",
											),
										),
							),
							Effect.mapError((cause) => make_capture_error(path, cause)),
						),
			CreatePrivate: (path) =>
				platform.kind === "posix"
					? Effect.scoped(
							Effect.gen(function* () {
								const file = yield* file_system.open(path, {
									flag: "wx",
									mode: 0o600,
								});
								return Option.some(
									yield* ReadPrivateFileIdentity(
										yield* FileDescriptorOf(file, "private file identity"),
									),
								);
							}),
						).pipe(
							Effect.catch((cause) =>
								cause instanceof PlatformError.PlatformError &&
								cause.reason._tag === "AlreadyExists"
									? Effect.succeed(Option.none<PrivateFileIdentity>())
									: Effect.fail(make_create_error(path, cause)),
							),
						)
					: RunWindowsAclCommand(
							child_process_spawner,
							make_windows_acl_command(path, powershell_create_private_file_script),
						).pipe(
							Effect.flatMap((output) => ParseCreatedWindowsIdentity(path, output)),
							Effect.mapError((cause) =>
								cause instanceof PrivateFilePermissionsCreateError
									? cause
									: make_create_error(path, cause),
							),
						),
			Restrict: (path) =>
				platform.kind === "posix"
					? file_system
							.chmod(path, 0o600)
							.pipe(Effect.mapError((cause) => make_restrict_error(path, cause)))
					: RunWindowsAclCommand(
							child_process_spawner,
							make_windows_acl_command(path, powershell_restrict_acl_script),
						).pipe(
							Effect.asVoid,
							Effect.mapError((cause) => make_restrict_error(path, cause)),
						),
			RestrictDirectory: (path) =>
				platform.kind === "posix"
					? file_system
							.chmod(path, 0o700)
							.pipe(Effect.mapError((cause) => make_restrict_error(path, cause)))
					: RunWindowsAclCommand(
							child_process_spawner,
							make_windows_acl_command(
								path,
								powershell_restrict_directory_acl_script,
							),
						).pipe(
							Effect.asVoid,
							Effect.mapError((cause) => make_restrict_error(path, cause)),
						),
			RestrictOwned: (path, identity) =>
				(platform.kind === "posix"
					? ApplyPosixOwned(file_system, path, identity, 0o600)
					: ApplyWindowsOwned(
							child_process_spawner,
							path,
							identity,
							powershell_owned_restrict_acl_script,
						)
				).pipe(Effect.mapError((cause) => make_restrict_error(path, cause))),
			Restore: (path, snapshot) =>
				platform.kind === "posix"
					? snapshot._tag === "PosixPrivateFilePermissionsSnapshot"
						? file_system
								.chmod(path, snapshot.mode)
								.pipe(Effect.mapError((cause) => make_restore_error(path, cause)))
						: Effect.fail(
								new PrivateFilePermissionsSnapshotPlatformMismatchError({
									path,
									platform: platform.kind,
									snapshot,
								}),
							)
					: snapshot._tag === "WindowsPrivateFilePermissionsSnapshot"
						? RunWindowsAclCommand(
								child_process_spawner,
								make_windows_acl_command(path, powershell_restore_acl_script, {
									sddl: snapshot.sddl,
								}),
							).pipe(
								Effect.asVoid,
								Effect.mapError((cause) => make_restore_error(path, cause)),
							)
						: Effect.fail(
								new PrivateFilePermissionsSnapshotPlatformMismatchError({
									path,
									platform: platform.kind,
									snapshot,
								}),
							),
			RestoreOwned: (path, identity, snapshot) =>
				platform.kind === "posix"
					? snapshot._tag === "PosixPrivateFilePermissionsSnapshot"
						? ApplyPosixOwned(file_system, path, identity, snapshot.mode).pipe(
								Effect.mapError((cause) => make_restore_error(path, cause)),
							)
						: Effect.fail(
								new PrivateFilePermissionsSnapshotPlatformMismatchError({
									path,
									platform: platform.kind,
									snapshot,
								}),
							)
					: snapshot._tag === "WindowsPrivateFilePermissionsSnapshot"
						? ApplyWindowsOwned(
								child_process_spawner,
								path,
								identity,
								powershell_owned_restore_acl_script,
								snapshot.sddl,
							).pipe(Effect.mapError((cause) => make_restore_error(path, cause)))
						: Effect.fail(
								new PrivateFilePermissionsSnapshotPlatformMismatchError({
									path,
									platform: platform.kind,
									snapshot,
								}),
							),
		};
	}),
);

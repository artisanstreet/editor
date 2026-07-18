import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { platform } from "node:process";

import { NodeChildProcessSpawner, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Layer, Option } from "effect";
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_private_file_permissions_layer,
	PrivateFilePermissions,
	PrivateFilePermissionsPlatform,
	ReadPrivateFileIdentity,
	WindowsPrivateFilePermissionsSnapshot,
} from "../../modules/backend/src/model-behaviour/private-file-permissions";

const roots: Array<string> = [];
const node_platform = NodeChildProcessSpawner.layer.pipe(
	Layer.provideMerge(NodeFileSystem.layer),
	Layer.provideMerge(NodePath.layer),
);

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan private permissions "));

	roots.push(root);

	return root;
}

function make_private_file_permissions() {
	const platform_kind = platform === "win32" ? "win32" : "posix";
	const dependencies = Layer.mergeAll(
		node_platform,
		Layer.succeed(PrivateFilePermissionsPlatform, { kind: platform_kind }),
	);
	const layer = Layer.provide(make_private_file_permissions_layer, dependencies);

	return Effect.runPromise(Effect.service(PrivateFilePermissions).pipe(Effect.provide(layer)));
}

async function read_identity(path: string) {
	const file = await fs.open(path, "r+");

	try {
		return await Effect.runPromise(ReadPrivateFileIdentity(file.fd));
	} finally {
		await file.close();
	}
}

function ReadWindowsAcl(path: string) {
	const script = [
		"$acl = if ([System.IO.Directory]::Exists($env:ARTISAN_PRIVATE_FILE_PATH)) { [System.IO.Directory]::GetAccessControl($env:ARTISAN_PRIVATE_FILE_PATH) } else { [System.IO.File]::GetAccessControl($env:ARTISAN_PRIVATE_FILE_PATH) }",
		"$current_user = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Translate([System.Security.Principal.NTAccount])",
		"$full_control = [System.Security.AccessControl.FileSystemRights]::FullControl",
		'$has_current_user_full_control = [bool]($acl.Access | Where-Object { $_.IdentityReference -eq $current_user -and $_.AccessControlType -eq "Allow" -and ($_.FileSystemRights -band $full_control) -eq $full_control })',
		"[pscustomobject]@{ protected = $acl.AreAccessRulesProtected; has_current_user_full_control = $has_current_user_full_control } | ConvertTo-Json -Compress",
	].join("; ");
	const command = ChildProcess.make(
		"powershell.exe",
		["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
		{
			env: { ARTISAN_PRIVATE_FILE_PATH: path },
			extendEnv: true,
			shell: false,
		},
	);

	return Effect.gen(function* () {
		const child_process_spawner = yield* ChildProcessSpawner.ChildProcessSpawner;
		const output = yield* Effect.scoped(child_process_spawner.string(command));

		return JSON.parse(output) as {
			readonly has_current_user_full_control: boolean;
			readonly protected: boolean;
		};
	});
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("PrivateFilePermissions", () => {
	it("creates an empty private file without overwriting an occupied path", async () => {
		const root = await make_root();
		const path = join(root, "created.toml");
		const permissions = await make_private_file_permissions();

		const created = await Effect.runPromise(permissions.CreatePrivate(path));

		expect(Option.isSome(created)).toBe(true);
		expect(await fs.readFile(path, "utf8")).toBe("");

		if (platform === "win32") {
			const acl = await Effect.runPromise(
				ReadWindowsAcl(path).pipe(Effect.provide(node_platform)),
			);

			expect(acl).toEqual({ has_current_user_full_control: true, protected: true });
		} else {
			expect((await fs.stat(path)).mode & 0o777).toBe(0o600);
		}

		const occupied = await Effect.runPromise(permissions.CreatePrivate(path));

		expect(Option.isNone(occupied)).toBe(true);
		expect(await fs.readFile(path, "utf8")).toBe("");
	});

	it("restricts a private directory to the current user", async () => {
		const root = await make_root();
		const path = join(root, "backups");
		const permissions = await make_private_file_permissions();

		await fs.mkdir(path, { mode: 0o777 });
		await Effect.runPromise(permissions.RestrictDirectory(path));

		if (platform === "win32") {
			const acl = await Effect.runPromise(
				ReadWindowsAcl(path).pipe(Effect.provide(node_platform)),
			);

			expect(acl).toEqual({ has_current_user_full_control: true, protected: true });
		} else {
			expect((await fs.stat(path)).mode & 0o777).toBe(0o700);
		}
	});

	it("changes permissions only while the exact owned file remains at the path", async () => {
		const root = await make_root();
		const path = join(root, "owned.toml");
		const permissions = await make_private_file_permissions();

		await fs.writeFile(path, "secret", { mode: 0o764 });

		const snapshot = await Effect.runPromise(permissions.Capture(path));
		const identity = await read_identity(path);
		const wrong_identity = { ...identity, inode: identity.inode + 1n };
		const rejected = await Effect.runPromise(permissions.RestrictOwned(path, wrong_identity));

		expect(rejected).toBe(false);
		expect(await Effect.runPromise(permissions.Capture(path))).toEqual(snapshot);

		const restricted = await Effect.runPromise(permissions.RestrictOwned(path, identity));

		expect(restricted).toBe(true);

		if (platform === "win32") {
			const acl = await Effect.runPromise(
				ReadWindowsAcl(path).pipe(Effect.provide(node_platform)),
			);

			expect(acl).toEqual({ has_current_user_full_control: true, protected: true });
		} else {
			expect((await fs.stat(path)).mode & 0o777).toBe(0o600);
		}

		const restored = await Effect.runPromise(
			permissions.RestoreOwned(path, identity, snapshot),
		);

		expect(restored).toBe(true);
		expect(await Effect.runPromise(permissions.Capture(path))).toEqual(snapshot);
	});

	it.skipIf(platform === "win32")("captures, restricts, and restores a POSIX mode", async () => {
		const root = await make_root();
		const path = join(root, "private.toml");
		const permissions = await make_private_file_permissions();

		await fs.writeFile(path, "secret");
		await fs.chmod(path, 0o764);

		const snapshot = await Effect.runPromise(permissions.Capture(path));
		await Effect.runPromise(permissions.Restrict(path));

		expect((await fs.stat(path)).mode & 0o777).toBe(0o600);

		await Effect.runPromise(permissions.Restore(path, snapshot));

		expect(snapshot).toMatchObject({
			_tag: "PosixPrivateFilePermissionsSnapshot",
			mode: 0o764,
		});
		expect((await fs.stat(path)).mode & 0o7777).toBe(0o764);
	});

	it.skipIf(platform !== "win32")(
		"captures, restricts, and restores a DACL for a path with PowerShell metacharacters",
		async () => {
			const root = await make_root();
			const path = join(root, "private file; $(Write-Error injected).toml");
			const permissions = await make_private_file_permissions();

			await fs.writeFile(path, "secret");

			const snapshot = await Effect.runPromise(permissions.Capture(path));
			await Effect.runPromise(permissions.Restrict(path));

			const acl = await Effect.runPromise(
				ReadWindowsAcl(path).pipe(Effect.provide(node_platform)),
			);

			expect(acl).toEqual({ has_current_user_full_control: true, protected: true });
			expect(await fs.readFile(path, "utf8")).toBe("secret");

			await Effect.runPromise(permissions.Restore(path, snapshot));

			const restored_snapshot = await Effect.runPromise(permissions.Capture(path));

			expect(snapshot).toBeInstanceOf(WindowsPrivateFilePermissionsSnapshot);
			expect(restored_snapshot).toEqual(snapshot);
		},
	);
});

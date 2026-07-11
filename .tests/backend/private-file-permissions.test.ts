import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { platform } from "node:process";

import { NodeServices } from "../../modules/backend/node_modules/@effect/platform-node/dist/index.js";
import { Effect, Layer } from "effect";
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_private_file_permissions_layer,
	PrivateFilePermissions,
	PrivateFilePermissionsPlatform,
} from "../../modules/backend/src/model-behaviour/private-file-permissions";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan private permissions "));

	roots.push(root);

	return root;
}

function make_private_file_permissions() {
	const platform_kind = platform === "win32" ? "win32" : "posix";
	const dependencies = Layer.mergeAll(
		NodeServices.layer,
		Layer.succeed(PrivateFilePermissionsPlatform, { kind: platform_kind }),
	);
	const layer = Layer.provide(make_private_file_permissions_layer, dependencies);

	return Effect.runPromise(Effect.service(PrivateFilePermissions).pipe(Effect.provide(layer)));
}

function ReadWindowsAcl(path: string) {
	const script = [
		"$acl = Get-Acl -LiteralPath $env:ARTISAN_PRIVATE_FILE_PATH",
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
	it.skipIf(platform === "win32")("sets POSIX private files to mode 0600", async () => {
		const root = await make_root();
		const path = join(root, "private.toml");
		const permissions = await make_private_file_permissions();

		await fs.writeFile(path, "secret", { mode: 0o644 });
		await Effect.runPromise(permissions.Restrict(path));

		expect((await fs.stat(path)).mode & 0o777).toBe(0o600);
	});

	it.skipIf(platform !== "win32")(
		"uses a protected DACL for a path with spaces and PowerShell metacharacters",
		async () => {
			const root = await make_root();
			const path = join(root, "private file; $(Write-Error injected).toml");
			const permissions = await make_private_file_permissions();

			await fs.writeFile(path, "secret");
			await Effect.runPromise(permissions.Restrict(path));

			const acl = await Effect.runPromise(
				ReadWindowsAcl(path).pipe(Effect.provide(NodeServices.layer)),
			);

			expect(acl).toEqual({ has_current_user_full_control: true, protected: true });
			expect(await fs.readFile(path, "utf8")).toBe("secret");
		},
	);
});

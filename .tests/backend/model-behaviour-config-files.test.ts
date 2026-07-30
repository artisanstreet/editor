import { createHash } from "node:crypto";
import { promises as fs } from "node:fs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeChildProcessSpawner, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Layer, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	ModelBehaviourConfigFiles,
	make_model_behaviour_config_files_layer,
	make_model_behaviour_config_files_platform_layer,
} from "../../modules/backend/src/model-behaviour/config-files";
import {
	make_private_file_permissions_layer,
	PrivateFilePermissions,
	PrivateFilePermissionsPlatform,
	PrivateFilePermissionsRestrictError,
	PrivateFilePermissionsRestoreError,
} from "../../modules/backend/src/model-behaviour/private-file-permissions";

const directories: Array<string> = [];
const node_platform = NodeChildProcessSpawner.layer.pipe(
	Layer.provideMerge(NodeFileSystem.layer),
	Layer.provideMerge(NodePath.layer),
);

function hash(content: string) {
	return createHash("sha256").update(Buffer.from(content, "utf8")).digest("hex");
}

async function make_directory() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-model-behaviour-config-"));

	directories.push(directory);

	return directory;
}

async function make_files(hooks = {}) {
	return Effect.runPromise(
		Effect.service(ModelBehaviourConfigFiles).pipe(
			Effect.provide(make_model_behaviour_config_files_layer(hooks)),
		),
	);
}

async function make_files_with_permissions(permissions: PrivateFilePermissions["Service"]) {
	const layer = make_model_behaviour_config_files_platform_layer().pipe(
		Layer.provide(Layer.succeed(PrivateFilePermissions, permissions)),
		Layer.provide(node_platform),
	);

	return Effect.runPromise(Effect.service(ModelBehaviourConfigFiles).pipe(Effect.provide(layer)));
}

async function make_permissions() {
	const platform = Layer.succeed(PrivateFilePermissionsPlatform, {
		kind: process.platform === "win32" ? "win32" : "posix",
	});
	const layer = make_private_file_permissions_layer.pipe(
		Layer.provide(platform),
		Layer.provide(node_platform),
	);

	return Effect.runPromise(Effect.service(PrivateFilePermissions).pipe(Effect.provide(layer)));
}

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("ModelBehaviourConfigFiles", () => {
	it("reads an absent file and creates a missing target", async () => {
		const directory = await make_directory();
		const path = join(directory, "nested folder", "config.toml");
		const files = await make_files();

		expect(await Effect.runPromise(files.Read(path))).toEqual(Option.none());

		const result = await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory: join(directory, "backups"),
				backup_name: "before.toml",
				content: "value = 1\n",
				path,
			}),
		);

		expect(result).toMatchObject({ _tag: "Written", content_hash: hash("value = 1\n"), path });
		expect(await readFile(path, "utf8")).toBe("value = 1\n");
	});

	it("conditionally replaces an existing file and makes a recoverable backup", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const backups_directory = join(directory, "backups");
		const files = await make_files();
		const original = 'model = "one"\r\n';

		await writeFile(path, original, { encoding: "utf8", mode: 0o640 });
		const result = await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory,
				backup_name: "config.before.toml",
				content: 'model = "two"\n',
				expected_content_hash: hash(original),
				path,
			}),
		);

		expect(result._tag).toBe("Written");
		expect(
			result.backup_path?.startsWith(join(backups_directory, "config.before.toml.original-")),
		).toBe(true);
		expect(await readFile(result.backup_path!, "utf8")).toBe(original);
	});

	it("does not overwrite a predictable backup-path collision", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const backups_directory = join(directory, "backups");
		const files = await make_files();
		const original = "value = 1\n";
		const collision_path = join(backups_directory, "before.toml");

		await writeFile(path, original, "utf8");
		await fs.mkdir(backups_directory, { recursive: true });
		await writeFile(collision_path, "external backup\n", "utf8");

		const result = await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory,
				backup_name: "before.toml",
				content: "value = 2\n",
				expected_content_hash: hash(original),
				path,
			}),
		);

		expect(result._tag).toBe("Written");
		expect(result.backup_path).not.toBe(collision_path);
		expect(await readFile(collision_path, "utf8")).toBe("external backup\n");
	});

	it("resumes from a matching operation backup after the target is moved", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const backups_directory = join(directory, "backups");
		const original = "value = 1\n";
		const interrupted_files = await make_files({
			after_claim: async () => {
				throw new Error("simulated process interruption");
			},
		});

		await writeFile(path, original, "utf8");
		const interrupted = await Effect.runPromise(
			interrupted_files
				.ReplaceAtomic({
					backups_directory,
					backup_name: "before.toml",
					content: "value = 2\n",
					expected_content_hash: hash(original),
					path,
				})
				.pipe(Effect.exit),
		);
		const files = await make_files();
		const retry = await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory,
				backup_name: "before.toml",
				content: "value = 2\n",
				expected_content_hash: hash(original),
				path,
			}),
		);

		expect(interrupted._tag).toBe("Failure");
		expect(retry._tag).toBe("Written");
		expect(await readFile(path, "utf8")).toBe("value = 2\n");
		expect(await readFile(retry.backup_path!, "utf8")).toBe(original);
	});

	it("returns Changed without clobbering a value raced before publication", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const original = "value = 1\n";
		const files = await make_files({
			before_replace: async () => writeFile(path, "raced\n", "utf8"),
		});

		await writeFile(path, original, "utf8");
		const result = await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory: join(directory, "backups"),
				backup_name: "before.toml",
				content: "value = 2\n",
				expected_content_hash: hash(original),
				path,
			}),
		);

		expect(result._tag).toBe("Changed");
		expect(result.content).toBe("raced\n");
		expect(await readFile(path, "utf8")).toBe("raced\n");
	});

	it("preserves a same-content file created by another process", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const replacement = "value = 2\n";
		const files = await make_files({
			after_claim: async () => writeFile(path, replacement, "utf8"),
		});

		const result = await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory: join(directory, "backups"),
				backup_name: "before.toml",
				content: replacement,
				path,
			}),
		);

		expect(result._tag).toBe("Changed");
		expect(await readFile(path, "utf8")).toBe(replacement);
	});

	it("preserves an external edit created after the expected revision is claimed", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const original = "value = 1\n";
		const raced = "external = true\n";
		const files = await make_files({
			after_claim: async () => writeFile(path, raced, "utf8"),
		});

		await writeFile(path, original, "utf8");

		const result = await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory: join(directory, "backups"),
				backup_name: "before.toml",
				content: "value = 2\n",
				expected_content_hash: hash(original),
				path,
			}),
		);

		expect(result._tag).toBe("Changed");
		expect(result.content).toBe(raced);
		expect(await readFile(path, "utf8")).toBe(raced);
	});

	it("restores the original bytes after a forced publication failure", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const original = "old\r\n";
		const files = await make_files({
			after_replace: async () => {
				throw new Error("forced publication failure");
			},
		});

		await writeFile(path, original, { encoding: "utf8", mode: 0o640 });
		const failure = await Effect.runPromise(
			files
				.ReplaceAtomic({
					backups_directory: join(directory, "backups"),
					backup_name: "before.toml",
					content: "new\n",
					expected_content_hash: hash(original),
					path,
				})
				.pipe(Effect.exit),
		);

		expect(failure._tag).toBe("Failure");
		expect(await readFile(path, "utf8")).toBe(original);

		if (process.platform !== "win32") {
			expect((await fs.stat(path)).mode & 0o777).toBe(0o640);
		}
	});

	it("restores the original after a private target write fails post-truncate", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const original = "old\n";
		const files = await make_files({
			after_private_truncate: async () => {
				throw new Error("forced target write failure");
			},
		});

		await writeFile(path, original, { encoding: "utf8", mode: 0o640 });

		const failure = await Effect.runPromise(
			files
				.ReplaceAtomic({
					backups_directory: join(directory, "backups"),
					backup_name: "before.toml",
					content: "new\n",
					expected_content_hash: hash(original),
					path,
				})
				.pipe(Effect.exit),
		);

		expect(failure._tag).toBe("Failure");
		expect(await readFile(path, "utf8")).toBe(original);

		if (process.platform !== "win32") {
			expect((await fs.stat(path)).mode & 0o777).toBe(0o640);
		}
	});

	it("does not delete an external replacement while rolling back publication", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const original = "old\n";
		const external = "external replacement\n";
		const files = await make_files({
			after_replace: async () => {
				throw new Error("forced publication failure");
			},
			before_rollback: async () => {
				await rm(path);
				await writeFile(path, external, { encoding: "utf8", mode: 0o644 });
			},
		});

		await writeFile(path, original, "utf8");
		const failure = await Effect.runPromise(
			files
				.ReplaceAtomic({
					backups_directory: join(directory, "backups"),
					backup_name: "before.toml",
					content: "new\n",
					expected_content_hash: hash(original),
					path,
				})
				.pipe(Effect.exit),
		);

		expect(failure._tag).toBe("Failure");
		expect(await readFile(path, "utf8")).toBe(external);

		if (process.platform !== "win32") {
			expect((await fs.stat(path)).mode & 0o777).toBe(0o644);
		}
	});

	it("does not create temporary or claim sidecars for a successful publication", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const files = await make_files();

		const result = await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory: join(directory, "backups"),
				backup_name: "before.toml",
				content: "new\n",
				path,
			}),
		);

		expect(result._tag).toBe("Written");
		expect(await readFile(path, "utf8")).toBe("new\n");
		expect(await fs.readdir(directory)).not.toEqual(
			expect.arrayContaining([expect.stringMatching(/\.artisan-(?:claim|config|dispose)-/)]),
		);
	});

	it("does not create temporary or claim sidecars while rolling back", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const original = "old\n";
		const files = await make_files({
			after_replace: async () => {
				throw new Error("forced publication failure");
			},
		});

		await writeFile(path, original, "utf8");
		const failure = await Effect.runPromise(
			files
				.ReplaceAtomic({
					backups_directory: join(directory, "backups"),
					backup_name: "before.toml",
					content: "new\n",
					expected_content_hash: hash(original),
					path,
				})
				.pipe(Effect.exit),
		);

		expect(failure._tag).toBe("Failure");
		expect(await readFile(path, "utf8")).toBe(original);
		expect(await fs.readdir(directory)).not.toEqual(
			expect.arrayContaining([expect.stringMatching(/\.artisan-(?:claim|config|dispose)-/)]),
		);
	});

	it("erases a newly created target in place when publication fails", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const backups_directory = join(directory, "backups");
		const files = await make_files({
			after_replace: async () => {
				throw new Error("forced publication failure");
			},
		});

		const failure = await Effect.runPromise(
			files
				.ReplaceAtomic({
					backups_directory,
					backup_name: "before.toml",
					content: "new secret\n",
					path,
				})
				.pipe(Effect.exit),
		);

		expect(failure._tag).toBe("Failure");
		expect(await readFile(path, "utf8")).toBe("");
		expect(await fs.readdir(backups_directory)).not.toEqual(
			expect.arrayContaining([expect.stringMatching(/\.failed-/)]),
		);
	});

	it("keeps restored bytes private when rollback cannot restore the original permissions", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const original = 'api_key = "secret"\n';
		const permissions = await make_permissions();
		const files = await make_files_with_permissions({
			...permissions,
			RestoreOwned: (restore_path) =>
				Effect.fail(
					new PrivateFilePermissionsRestoreError({
						cause: new Error("forced permission restoration failure"),
						path: restore_path,
					}),
				),
		});

		await writeFile(path, original, { encoding: "utf8", mode: 0o644 });

		const failure = await Effect.runPromise(
			files
				.ReplaceAtomic({
					backups_directory: join(directory, "backups"),
					backup_name: "before.toml",
					content: 'api_key = "replacement"\n',
					expected_content_hash: hash(original),
					path,
				})
				.pipe(Effect.exit),
		);

		expect(failure._tag).toBe("Failure");
		expect(await readFile(path, "utf8")).toBe(original);

		if (process.platform === "win32") {
			const snapshot = await Effect.runPromise(permissions.Capture(path));

			expect(snapshot).toMatchObject({
				_tag: "WindowsPrivateFilePermissionsSnapshot",
				sddl: expect.stringContaining("D:P"),
			});
		} else {
			expect((await fs.stat(path)).mode & 0o777).toBe(0o600);
		}
	});

	it("writes replacement bytes privately before any rollback permission operation", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const original = 'api_key = "secret"\n';
		const replacement = 'api_key = "replacement"\n';
		const permissions = await make_permissions();
		const files = await make_files_with_permissions({
			...permissions,
			RestrictOwned: (restrict_path, identity) =>
				restrict_path === path
					? Effect.fail(
							new PrivateFilePermissionsRestrictError({
								cause: new Error("forced rollback restriction failure"),
								path: restrict_path,
							}),
						)
					: permissions.RestrictOwned(restrict_path, identity),
			RestoreOwned: (restore_path) =>
				Effect.fail(
					new PrivateFilePermissionsRestoreError({
						cause: new Error("forced publication permission failure"),
						path: restore_path,
					}),
				),
		});

		await writeFile(path, original, { encoding: "utf8", mode: 0o644 });

		const failure = await Effect.runPromise(
			files
				.ReplaceAtomic({
					backups_directory: join(directory, "backups"),
					backup_name: "before.toml",
					content: replacement,
					expected_content_hash: hash(original),
					path,
				})
				.pipe(Effect.exit),
		);

		expect(failure._tag).toBe("Failure");
		expect(await readFile(path, "utf8")).toBe(replacement);

		if (process.platform === "win32") {
			const snapshot = await Effect.runPromise(permissions.Capture(path));

			expect(snapshot).toMatchObject({
				_tag: "WindowsPrivateFilePermissionsSnapshot",
				sddl: expect.stringContaining("D:P"),
			});
		} else {
			expect((await fs.stat(path)).mode & 0o777).toBe(0o600);
		}
	});

	it("distinguishes exact bytes rather than normalized text", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const files = await make_files();
		const content = "value = 1\r\n";

		await writeFile(path, content, "utf8");
		const result = await Effect.runPromise(files.Read(path));

		expect(Option.getOrThrow(result).content_hash).toBe(hash(content));
		expect(Option.getOrThrow(result).content_hash).not.toBe(hash("value = 1\n"));
	});

	it("preserves an existing Windows ACL and protects a newly created target", async () => {
		if (process.platform !== "win32") {
			return;
		}

		const directory = await make_directory();
		const backups_directory = join(directory, "backups");
		const existing_path = join(directory, "existing.toml");
		const new_path = join(directory, "new.toml");
		const original = "value = 1\n";
		const files = await make_files();
		const permissions = await make_permissions();

		await writeFile(existing_path, original, "utf8");
		await Effect.runPromise(permissions.Restrict(existing_path));

		const original_permissions = await Effect.runPromise(permissions.Capture(existing_path));

		await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory,
				backup_name: "existing.toml",
				content: "value = 2\n",
				expected_content_hash: hash(original),
				path: existing_path,
			}),
		);
		await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory,
				backup_name: "new.toml",
				content: "value = 1\n",
				path: new_path,
			}),
		);

		const replaced_permissions = await Effect.runPromise(permissions.Capture(existing_path));
		const new_permissions = await Effect.runPromise(permissions.Capture(new_path));
		const backups_permissions = await Effect.runPromise(permissions.Capture(backups_directory));

		expect(replaced_permissions).toEqual(original_permissions);
		expect(new_permissions).toMatchObject({
			_tag: "WindowsPrivateFilePermissionsSnapshot",
			sddl: expect.stringContaining("D:P"),
		});
		expect(backups_permissions).toMatchObject({
			_tag: "WindowsPrivateFilePermissionsSnapshot",
			sddl: expect.stringContaining("D:P"),
		});
	}, 30_000);

	it("does not change a target substituted before permission application", async () => {
		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const original = "value = 1\n";
		const external = "external = true\n";
		const permissions = await make_permissions();
		let external_permissions: unknown;
		const files = await make_files({
			before_permissions: async (permissions_path: string) => {
				if (permissions_path !== path) {
					return;
				}

				await rm(path);
				await writeFile(path, external, { encoding: "utf8", mode: 0o644 });
				external_permissions = await Effect.runPromise(permissions.Capture(path));
			},
		});

		await writeFile(path, original, "utf8");
		const result = await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory: join(directory, "backups"),
				backup_name: "before.toml",
				content: "value = 2\n",
				expected_content_hash: hash(original),
				path,
			}),
		);
		const final_permissions = await Effect.runPromise(permissions.Capture(path));

		expect(result._tag).toBe("Changed");
		expect(await readFile(path, "utf8")).toBe(external);
		expect(final_permissions).toEqual(external_permissions);
	});

	it("preserves target permissions while keeping credential-bearing backups private", async () => {
		if (process.platform === "win32") {
			return;
		}

		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const backups_directory = join(directory, "backups");
		const original = 'api_key = "secret"\n';
		const files = await make_files();

		await writeFile(path, original, { encoding: "utf8", mode: 0o640 });

		const result = await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory,
				backup_name: "before.toml",
				content: 'api_key = "secret"\nvalue = 2\n',
				expected_content_hash: hash(original),
				path,
			}),
		);
		const target_mode = (await fs.stat(path)).mode & 0o777;
		const backup_mode = (await fs.stat(result.backup_path!)).mode & 0o777;

		expect(target_mode).toBe(0o640);
		expect(backup_mode).toBe(0o600);
	});

	it("makes a reused matching backup private", async () => {
		if (process.platform === "win32") {
			return;
		}

		const directory = await make_directory();
		const path = join(directory, "config.toml");
		const backups_directory = join(directory, "backups");
		const original = 'api_key = "secret"\n';
		const interrupted_files = await make_files({
			after_claim: async () => {
				throw new Error("simulated process interruption");
			},
		});

		await writeFile(path, original, "utf8");
		await Effect.runPromise(
			interrupted_files
				.ReplaceAtomic({
					backups_directory,
					backup_name: "before.toml",
					content: 'api_key = "secret"\nvalue = 2\n',
					expected_content_hash: hash(original),
					path,
				})
				.pipe(Effect.exit),
		);

		const backup_name = (await fs.readdir(backups_directory)).find(
			(name) => name.startsWith("before.toml.original-") && !name.endsWith(".json"),
		)!;
		const backup_path = join(backups_directory, backup_name);

		await fs.chmod(backup_path, 0o644);

		const files = await make_files();

		const result = await Effect.runPromise(
			files.ReplaceAtomic({
				backups_directory,
				backup_name: "before.toml",
				content: 'api_key = "secret"\nvalue = 2\n',
				expected_content_hash: hash(original),
				path,
			}),
		);

		expect(result._tag).toBe("Written");
		expect((await fs.stat(backup_path)).mode & 0o777).toBe(0o600);
	});
});

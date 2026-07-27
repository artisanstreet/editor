import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { describe, expect, it } from "vitest";
import { Effect, Layer, ManagedRuntime } from "effect";

import {
	Activation,
	ActivationInvalidVersion,
	ActivationRollbackUnavailable,
	ActivationVersionUnavailable,
	make_activation_layer,
} from "../../modules/distribution/src/activation";

const WithActivation = async (
	operation: (activation: Activation["Service"], root: string) => Promise<void>,
) => {
	const root = await mkdtemp(join(tmpdir(), "artisan-activation-"));
	const runtime = ManagedRuntime.make(
		make_activation_layer(root).pipe(
			Layer.provide(NodeFileSystem.layer),
			Layer.provide(NodePath.layer),
		),
	);

	try {
		await operation(await runtime.runPromise(Activation), root);
	} finally {
		await runtime.dispose();
		await rm(root, { force: true, recursive: true });
	}
};

describe("Activation", () => {
	it("creates the managed binary layout", async () => {
		await WithActivation(async (activation, root) => {
			const layout = await Effect.runPromise(activation.EnsureLayout());
			expect(layout).toEqual({
				root,
				bin: join(root, "bin"),
				versions: join(root, "versions"),
				staging: join(root, "staging"),
				current: join(root, "current"),
			});
			expect(
				await Promise.all(
					[layout.bin, layout.versions, layout.staging].map(async (path) =>
						(await stat(path)).isDirectory(),
					),
				),
			).toEqual([true, true, true]);
		});
	});

	it("atomically activates installed versions and preserves a rollback pointer", async () => {
		await WithActivation(async (activation, root) => {
			await mkdir(join(root, "versions", "0.1.0"), { recursive: true });
			await mkdir(join(root, "versions", "0.2.0"), { recursive: true });

			expect(await Effect.runPromise(activation.Activate("0.1.0"))).toEqual({
				format_version: 1,
				active_version: "0.1.0",
			});
			expect(await Effect.runPromise(activation.Activate("0.2.0"))).toEqual({
				format_version: 1,
				active_version: "0.2.0",
				previous_version: "0.1.0",
			});
			expect(JSON.parse(await readFile(join(root, "current"), "utf8"))).toMatchObject({
				active_version: "0.2.0",
				previous_version: "0.1.0",
			});

			expect(await Effect.runPromise(activation.Rollback())).toEqual({
				format_version: 1,
				active_version: "0.1.0",
				previous_version: "0.2.0",
			});
		});
	});

	it("rejects unsafe or unavailable versions without changing the active pointer", async () => {
		await WithActivation(async (activation, root) => {
			await mkdir(join(root, "versions", "0.1.0"), { recursive: true });
			await Effect.runPromise(activation.Activate("0.1.0"));
			const original = await readFile(join(root, "current"), "utf8");

			await expect(
				Effect.runPromise(activation.Activate("../escape")),
			).rejects.toBeInstanceOf(ActivationInvalidVersion);
			await expect(Effect.runPromise(activation.Activate("0.2.0"))).rejects.toBeInstanceOf(
				ActivationVersionUnavailable,
			);
			expect(await readFile(join(root, "current"), "utf8")).toBe(original);
		});
	});

	it("reports malformed and missing rollback state without deleting it", async () => {
		await WithActivation(async (activation, root) => {
			expect(await Effect.runPromise(activation.ReadActive())).toEqual({ _tag: "Absent" });
			await writeFile(join(root, "current"), "{broken", "utf8");
			expect(await Effect.runPromise(activation.ReadActive())).toMatchObject({
				_tag: "Malformed",
			});
			await expect(Effect.runPromise(activation.Rollback())).rejects.toBeInstanceOf(
				ActivationRollbackUnavailable,
			);
			expect(await readFile(join(root, "current"), "utf8")).toBe("{broken");
		});
	});
});

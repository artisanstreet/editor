import { Effect, FileSystem, Layer, Path } from "effect";
import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { describe, expect, it } from "vitest";

import {
	HostedProjectCloneDestination,
	make_hosted_project_clone_destination_layer,
} from "../../modules/backend/src/projects/hosted-project-clone-destination";

const platform_layer = Layer.mergeAll(NodeFileSystem.layer, NodePath.layer);

async function test_platform() {
	return Effect.runPromise(
		Effect.all({
			file_system: Effect.service(FileSystem.FileSystem),
			path_service: Effect.service(Path.Path),
		}).pipe(Effect.provide(platform_layer)),
	);
}

async function with_temporary_directory<A>(use: (root: string) => Promise<A>) {
	const { file_system } = await test_platform();
	const root = await Effect.runPromise(
		file_system.makeTempDirectory({ prefix: "artisan-clone-destination-test-" }),
	);

	try {
		return await use(root);
	} finally {
		await Effect.runPromise(file_system.remove(root, { recursive: true }));
	}
}

async function make_destination(projects_root?: string) {
	return Effect.runPromise(
		Effect.service(HostedProjectCloneDestination).pipe(
			Effect.provide(
				make_hosted_project_clone_destination_layer(
					projects_root === undefined ? {} : { projects_root },
				).pipe(Layer.provide(platform_layer)),
			),
		),
	);
}

describe("HostedProjectCloneDestination", () => {
	it("binds and holds one visible empty direct child while execution populates it", async () => {
		await with_temporary_directory(async (projects_root) => {
			const { file_system, path_service } = await test_platform();
			const destination_path = path_service.join(projects_root, "artisan-editor");

			await Effect.runPromise(file_system.makeDirectory(destination_path));

			const destination = await make_destination(projects_root);
			const plan = await Effect.runPromise(destination.Plan(destination_path));
			let observed_root: string | undefined;

			const result = await Effect.runPromise(
				destination.WithPinned(plan, (proof) =>
					Effect.gen(function* () {
						observed_root = proof.canonical_root;

						yield* file_system.makeDirectory(
							path_service.join(proof.canonical_root, ".git"),
						);

						return "populated" as const;
					}),
				),
			);
			const canonical_root = await Effect.runPromise(file_system.realPath(destination_path));

			expect(result).toBe("populated");
			expect(observed_root).toBe(canonical_root);
			expect(plan.canonical_root).toBe(canonical_root);
			expect(plan.projects_root).toBe(
				await Effect.runPromise(file_system.realPath(projects_root)),
			);
			expect(plan.projects_root_device).toMatch(/^(?:0|[1-9][0-9]*)$/u);
			expect(plan.projects_root_inode).toMatch(/^(?:0|[1-9][0-9]*)$/u);
			expect(plan.root_device).toMatch(/^(?:0|[1-9][0-9]*)$/u);
			expect(plan.root_inode).toMatch(/^(?:0|[1-9][0-9]*)$/u);
			expect(await Effect.runPromise(file_system.readDirectory(canonical_root))).toEqual([
				".git",
			]);
		});
	});

	it("rejects absent, nonempty, nested, and outside destinations", async () => {
		await with_temporary_directory(async (root) => {
			const { file_system, path_service } = await test_platform();
			const projects_root = path_service.join(root, "projects");
			const nested = path_service.join(projects_root, "nested", "child");
			const outside = path_service.join(root, "outside");
			const nonempty = path_service.join(projects_root, "nonempty");

			await Effect.runPromise(
				Effect.all([
					file_system.makeDirectory(nested, { recursive: true }),
					file_system.makeDirectory(outside),
					file_system.makeDirectory(nonempty),
				]),
			);
			await Effect.runPromise(
				file_system.writeFileString(
					path_service.join(nonempty, "occupied.txt"),
					"occupied",
				),
			);

			const destination = await make_destination(projects_root);
			const cases = [
				{
					path: path_service.join(projects_root, "absent"),
					reason: "destination_unavailable",
				},
				{ path: nonempty, reason: "destination_not_empty" },
				{ path: nested, reason: "invalid_destination" },
				{ path: outside, reason: "invalid_destination" },
			] as const;

			for (const test_case of cases) {
				const error = await Effect.runPromise(
					destination.Plan(test_case.path).pipe(Effect.flip),
				);

				expect(error).toMatchObject({ reason: test_case.reason });
			}
		});
	});

	it("fails closed when the projects root is replaced after planning", async () => {
		await with_temporary_directory(async (root) => {
			const { file_system, path_service } = await test_platform();
			const projects_root = path_service.join(root, "projects");
			const replaced_root = path_service.join(root, "projects-replaced");
			const destination_path = path_service.join(projects_root, "editor");

			await Effect.runPromise(
				file_system.makeDirectory(destination_path, { recursive: true }),
			);

			const destination = await make_destination(projects_root);
			const plan = await Effect.runPromise(destination.Plan(destination_path));
			let callback_count = 0;

			await Effect.runPromise(file_system.rename(projects_root, replaced_root));
			await Effect.runPromise(
				file_system.makeDirectory(destination_path, { recursive: true }),
			);

			const error = await Effect.runPromise(
				destination
					.WithPinned(plan, () =>
						Effect.sync(() => {
							callback_count += 1;
						}),
					)
					.pipe(Effect.flip),
			);

			expect(error).toMatchObject({ reason: "projects_root_unavailable" });
			expect(callback_count).toBe(0);
		});
	});

	it("rejects a destination replacement before entering execution", async () => {
		await with_temporary_directory(async (projects_root) => {
			const { file_system, path_service } = await test_platform();
			const destination_path = path_service.join(projects_root, "editor");

			await Effect.runPromise(file_system.makeDirectory(destination_path));

			const destination = await make_destination(projects_root);
			const plan = await Effect.runPromise(destination.Plan(destination_path));
			let callback_count = 0;

			await Effect.runPromise(file_system.remove(destination_path, { recursive: true }));
			await Effect.runPromise(file_system.makeDirectory(destination_path));

			const error = await Effect.runPromise(
				destination
					.WithPinned(plan, () =>
						Effect.sync(() => {
							callback_count += 1;
						}),
					)
					.pipe(Effect.flip),
			);

			expect(error).toMatchObject({ reason: "destination_unavailable" });
			expect(callback_count).toBe(0);
		});
	});

	it("rejects a destination replacement during execution", async () => {
		await with_temporary_directory(async (projects_root) => {
			const { file_system, path_service } = await test_platform();
			const destination_path = path_service.join(projects_root, "editor");

			await Effect.runPromise(file_system.makeDirectory(destination_path));

			const destination = await make_destination(projects_root);
			const plan = await Effect.runPromise(destination.Plan(destination_path));

			const error = await Effect.runPromise(
				destination
					.WithPinned(plan, () =>
						file_system
							.remove(destination_path, { recursive: true })
							.pipe(Effect.andThen(file_system.makeDirectory(destination_path))),
					)
					.pipe(Effect.flip),
			);

			expect(error).toMatchObject({ reason: "destination_unavailable" });
		});
	});

	it("rejects a projects-root replacement during execution", async () => {
		await with_temporary_directory(async (root) => {
			const { file_system, path_service } = await test_platform();
			const projects_root = path_service.join(root, "projects");
			const replaced_root = path_service.join(root, "projects-replaced");
			const destination_path = path_service.join(projects_root, "editor");

			await Effect.runPromise(
				file_system.makeDirectory(destination_path, { recursive: true }),
			);

			const destination = await make_destination(projects_root);
			const plan = await Effect.runPromise(destination.Plan(destination_path));

			const outcome = await Effect.runPromise(
				destination
					.WithPinned(plan, () =>
						file_system.rename(projects_root, replaced_root).pipe(
							Effect.andThen(
								file_system.makeDirectory(destination_path, {
									recursive: true,
								}),
							),
							Effect.as("replaced" as const),
							Effect.catch(() => Effect.succeed("blocked" as const)),
						),
					)
					.pipe(
						Effect.match({
							onFailure: (error) => ({ error, type: "rejected" as const }),
							onSuccess: (result) => ({ result, type: "completed" as const }),
						}),
					),
			);

			if (outcome.type === "completed") {
				expect(outcome.result).toBe("blocked");

				return;
			}

			expect(outcome.error).toMatchObject({ reason: "projects_root_unavailable" });
		});
	});

	it("preserves a visible destination when execution fails", async () => {
		await with_temporary_directory(async (projects_root) => {
			const { file_system, path_service } = await test_platform();
			const destination_path = path_service.join(projects_root, "editor");

			await Effect.runPromise(file_system.makeDirectory(destination_path));

			const destination = await make_destination(projects_root);
			const plan = await Effect.runPromise(destination.Plan(destination_path));

			const error = await Effect.runPromise(
				destination
					.WithPinned(plan, () => Effect.fail("execution_failed" as const))
					.pipe(Effect.flip),
			);

			expect(error).toBe("execution_failed");
			expect(await Effect.runPromise(file_system.exists(destination_path))).toBe(true);
		});
	});

	it("reports an unavailable projects root without touching the filesystem", async () => {
		const destination = await make_destination();
		const error = await Effect.runPromise(
			destination.Plan("C:\\Projects\\editor").pipe(Effect.flip),
		);

		expect(error).toMatchObject({ reason: "projects_root_unavailable" });
	});
});

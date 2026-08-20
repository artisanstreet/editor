import { Context, Effect, Exit, Layer, Scope } from "effect";
import { describe, expect, it } from "vitest";

import { ArtisanClient } from "@artisan/transport/client";

import {
	ProjectIdentityController,
	ProjectIdentityControllerLive,
} from "../../modules/frontend/src/lib/root/project-identity-controller";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";

describe("ProjectIdentityController", () => {
	it("loads project identities as one batch and retains them by project id", async () => {
		const requests: Array<ReadonlyArray<string>> = [];
		const scope = await Effect.runPromise(Scope.make());
		const layer = ProjectIdentityControllerLive.pipe(
			Layer.provide(
				Layer.succeed(ArtisanClient, {
					...FixtureArtisanClientService,
					GetProjectIdentities: (project_ids = []) => {
						requests.push(project_ids);
						return Effect.succeed({
							identities: project_ids.map((project_id) => ({
								host: "github" as const,
								kind: "repository" as const,
								project_id,
							})),
						});
					},
				}),
			),
		);
		const context = await Effect.runPromise(Layer.buildWithScope(layer, scope));
		const controller = Context.get(context, ProjectIdentityController);

		await Effect.runPromise(controller.Refresh(["project_1", "project_2"]));
		const current = await Effect.runPromise(controller.Current);

		expect(requests).toEqual([["project_1", "project_2"]]);
		expect(current.get("project_1")).toMatchObject({
			host: "github",
			kind: "repository",
		});
		expect(current.get("project_2")?.project_id).toBe("project_2");
		await Effect.runPromise(Scope.close(scope, Exit.void));
	});
});

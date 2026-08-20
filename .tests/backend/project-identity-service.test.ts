import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import type {
	Project,
	ProjectRepository,
	RepositoryRemote,
	RichLinkAssetMetadata,
} from "@artisan/protocol";

import { RepositoryService } from "../../modules/backend/src/git/repository-service";
import {
	RichLinkMetadata,
	type RichLinkMetadataError,
} from "../../modules/backend/src/preview/rich-link-metadata";
import {
	ProjectAvatarCandidatesFor,
	ProjectIdentityService,
	ProjectIdentityServiceLive,
} from "../../modules/backend/src/projects/project-identity-service";

const project: Project = {
	attached_at: "2026-08-20T10:00:00.000Z",
	display_name: "artisan-editor",
	project_id: "project_01",
	root_path: "C:\\Users\\sander\\Desktop\\artisan-editor",
	updated_at: "2026-08-20T10:00:00.000Z",
};

const remote = (host: RepositoryRemote["host"], web_url: string): RepositoryRemote => ({
	host,
	name: "origin",
	url: web_url,
	web_url,
});

const repository = (origin?: RepositoryRemote): ProjectRepository =>
	({
		branch: { name: "master", type: "attached" },
		...(origin === undefined ? {} : { default_remote: "origin" }),
		remotes: origin === undefined ? [] : [origin],
		state: "repository",
	}) as unknown as ProjectRepository;

type ResolveImage = (url: string) => Effect.Effect<RichLinkAssetMetadata, RichLinkMetadataError>;

const ResolveWith = (observed: ProjectRepository, resolve_image: ResolveImage) =>
	Effect.runPromise(
		Effect.gen(function* () {
			const service = yield* ProjectIdentityService;
			return yield* service.Resolve(project);
		}).pipe(
			Effect.provide(
				ProjectIdentityServiceLive.pipe(
					Layer.provide(
						Layer.mergeAll(
							Layer.succeed(RepositoryService, {
								Diff: () => Effect.die("not used"),
								Inspect: () => Effect.succeed(observed),
							}),
							Layer.succeed(RichLinkMetadata, {
								Resolve: () => Effect.die("not used"),
								ResolveImage: resolve_image,
							}),
						),
					),
				),
			),
		),
	);

describe("project identity service", () => {
	it("derives GitHub owner and GitLab project/namespace candidates in fallback order", () => {
		expect(
			ProjectAvatarCandidatesFor(remote("github", "https://github.com/openai/codex")),
		).toEqual([{ source: "owner", url: "https://github.com/openai.png?size=64" }]);
		expect(
			ProjectAvatarCandidatesFor(
				remote("gitlab", "https://gitlab.example.com/group/platform/editor"),
			),
		).toEqual([
			{
				source: "project",
				url: "https://gitlab.example.com/api/v4/projects/group%2Fplatform%2Feditor/avatar",
			},
			{
				source: "owner",
				url: "https://gitlab.example.com/api/v4/groups/group%2Fplatform/avatar",
			},
		]);
	});

	it("retains a verified provider image and reports its source", async () => {
		const requested: Array<string> = [];
		const identity = await ResolveWith(
			repository(remote("github", "https://github.com/openai/codex")),
			(url) => {
				requested.push(url);
				return Effect.succeed({
					asset_id: "sha256:avatar",
					bytes: 9,
					content_type: "image/png",
				});
			},
		);

		expect(requested).toEqual(["https://github.com/openai.png?size=64"]);
		expect(identity).toEqual({
			host: "github",
			image: {
				asset_id: "sha256:avatar",
				bytes: 9,
				content_type: "image/png",
				source: "owner",
			},
			kind: "repository",
			project_id: "project_01",
		});
	});

	it("uses a folder for repositories that never leave the machine", async () => {
		const identity = await ResolveWith(repository(), () => Effect.die("not used"));
		expect(identity).toEqual({ kind: "folder", project_id: "project_01" });
	});
});

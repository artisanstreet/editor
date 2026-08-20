import { Cache, Context, Effect, Layer, Option } from "effect";

import type {
	Project,
	ProjectIdentityImage,
	ProjectIdentitySource,
	RepositoryRemote,
} from "@artisan/protocol";

import { RepositoryService } from "../git/repository-service";
import { RichLinkMetadata } from "../preview/rich-link-metadata";

const maximum_retained_avatar_urls = 128;
const avatar_cache_ttl = "5 minutes";

interface AvatarCandidate {
	readonly source: ProjectIdentityImage["source"];
	readonly url: string;
}

const WebPathSegments = (web_url: string): ReadonlyArray<string> | undefined => {
	const parsed = URL.parse(web_url);
	if (parsed === null || parsed.protocol !== "https:") return undefined;
	const segments = parsed.pathname.split("/").filter((segment) => segment !== "");
	return segments.length >= 2 ? segments : undefined;
};

/**
 * Derives public, provider-owned avatar endpoints without exposing a remote URL
 * to the renderer. GitHub has no repository-avatar endpoint, so its owner is
 * the first meaningful image. GitLab can expose a project image and then a
 * namespace/group image.
 */
export const ProjectAvatarCandidatesFor = (
	remote: RepositoryRemote,
): ReadonlyArray<AvatarCandidate> => {
	if (remote.web_url === undefined) return [];
	const parsed = URL.parse(remote.web_url);
	const segments = WebPathSegments(remote.web_url);
	if (parsed === null || segments === undefined) return [];

	if (remote.host === "github") {
		return [
			{
				source: "owner",
				url: new URL(`/${encodeURIComponent(segments[0]!)}.png?size=64`, parsed.origin)
					.href,
			},
		];
	}

	if (remote.host === "gitlab") {
		const repository_path = segments.join("/");
		const namespace_path = segments.slice(0, -1).join("/");
		return [
			{
				source: "project",
				url: new URL(
					`/api/v4/projects/${encodeURIComponent(repository_path)}/avatar`,
					parsed.origin,
				).href,
			},
			{
				source: "owner",
				url: new URL(
					`/api/v4/groups/${encodeURIComponent(namespace_path)}/avatar`,
					parsed.origin,
				).href,
			},
		];
	}

	return [];
};

/** Resolves provider imagery while keeping local projects local and folder-shaped. */
export class ProjectIdentityService extends Context.Service<
	ProjectIdentityService,
	{
		readonly Resolve: (project: Project) => Effect.Effect<ProjectIdentitySource>;
	}
>()("Artisan/ProjectIdentityService") {}

export const ProjectIdentityServiceLive = Layer.effect(
	ProjectIdentityService,
	Effect.gen(function* () {
		const repositories = yield* RepositoryService;
		const rich_links = yield* RichLinkMetadata;
		const images = yield* Cache.makeWith(
			(url: string) => rich_links.ResolveImage(url).pipe(Effect.option),
			{
				capacity: maximum_retained_avatar_urls,
				timeToLive: () => avatar_cache_ttl,
			},
		);

		const Resolve = (project: Project): Effect.Effect<ProjectIdentitySource> =>
			repositories.Inspect(project.root_path).pipe(
				Effect.flatMap((repository): Effect.Effect<ProjectIdentitySource> => {
					if (repository.state === "not_repository") {
						return Effect.succeed({
							kind: "folder",
							project_id: project.project_id,
						} satisfies ProjectIdentitySource);
					}

					const remote = repository.remotes.find(
						(candidate) => candidate.name === repository.default_remote,
					);
					if (remote?.web_url === undefined) {
						return Effect.succeed({
							kind: "folder",
							project_id: project.project_id,
						} satisfies ProjectIdentitySource);
					}

					return Effect.gen(function* () {
						for (const candidate of ProjectAvatarCandidatesFor(remote)) {
							const image = yield* Cache.get(images, candidate.url);
							if (Option.isSome(image)) {
								return {
									host: remote.host,
									image: { ...image.value, source: candidate.source },
									kind: "repository",
									project_id: project.project_id,
								} satisfies ProjectIdentitySource;
							}
						}

						return {
							host: remote.host,
							kind: "repository",
							project_id: project.project_id,
						} satisfies ProjectIdentitySource;
					});
				}),
				Effect.catchCause(() =>
					Effect.succeed({
						kind: "folder",
						project_id: project.project_id,
					} satisfies ProjectIdentitySource),
				),
			);

		return ProjectIdentityService.of({ Resolve });
	}),
);

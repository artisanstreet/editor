import { Context, Effect, Schema } from "effect";

import {
	ProjectDiffMaximumProjects,
	OutboundControlEnvelope,
	type ProjectDetachEnvelope,
	type ProjectDiffQueryEnvelope,
	type ProjectDirectoryListQueryEnvelope,
	type ProjectDirectorySelectEnvelope,
	type ProjectListQueryEnvelope,
	type ProjectRepositoryQueryEnvelope,
	type RuntimeCatalogQueryEnvelope,
	type SessionDefaultsQueryEnvelope,
} from "@artisan/protocol";

import { RepositoryService } from "../../../git/repository-service";
import { ProjectCatalog } from "../../../projects/project-catalog";
import { ProjectDirectoryService } from "../../../projects/project-directory-service";
import { RuntimeCatalogService } from "../../../runtime/catalog";
import { RuntimeMetadata } from "../../../runtime/metadata";
import { SessionDefaultsService } from "../../../settings/session-defaults-service";
import type { ReadyState } from "../../connection-state";

export type ProjectQueryEnvelope =
	| ProjectDetachEnvelope
	| ProjectDiffQueryEnvelope
	| ProjectDirectoryListQueryEnvelope
	| ProjectDirectorySelectEnvelope
	| ProjectListQueryEnvelope
	| ProjectRepositoryQueryEnvelope
	| RuntimeCatalogQueryEnvelope
	| SessionDefaultsQueryEnvelope;

export class ConnectionResponseSink extends Context.Service<
	ConnectionResponseSink,
	{
		readonly Enqueue: (envelope: OutboundControlEnvelope) => Effect.Effect<void>;
		readonly EnqueueError: (
			current: ReadyState,
			code: string,
			message: string,
			retryable: boolean,
			correlation_id?: string,
		) => Effect.Effect<void>;
	}
>()("Artisan/Protocol/ConnectionResponseSink") {}

const literal = <Value extends string>(value: Value): Value => value;

export const MakeProjectQueryHandler = Effect.gen(function* () {
	const directories = yield* ProjectDirectoryService;
	const projects = yield* ProjectCatalog;
	const repositories = yield* RepositoryService;
	const defaults = yield* SessionDefaultsService;
	const runtime = yield* RuntimeCatalogService;
	const metadata = yield* RuntimeMetadata;
	const sink = yield* ConnectionResponseSink;

	const Respond = (
		query: ProjectQueryEnvelope,
		envelope: Omit<
			OutboundControlEnvelope,
			| "correlation_id"
			| "message_id"
			| "origin"
			| "protocol_version"
			| "schema_version"
			| "sent_at"
		>,
	) =>
		Effect.gen(function* () {
			const candidate: unknown = {
				...envelope,
				correlation_id: query.message_id,
				message_id: yield* metadata.MakeId("message"),
				origin: "backend",
				protocol_version: 1,
				schema_version: 1,
				sent_at: yield* metadata.Now,
			};
			const response = yield* Schema.decodeUnknownEffect(OutboundControlEnvelope)(candidate);
			yield* sink.Enqueue(response);
		});

	const Recover = (
		query: ProjectQueryEnvelope,
		current: ReadyState,
		code: string,
		message: string,
		retryable: boolean,
	) => sink.EnqueueError(current, code, message, retryable, query.message_id);

	const handlers = {
		"project.directory.list.query": (
			query: ProjectDirectoryListQueryEnvelope,
			current: ReadyState,
		) =>
			directories.List(query.payload).pipe(
				Effect.flatMap((payload) =>
					Respond(query, { kind: "project.directory.list.query.result", payload }),
				),
				Effect.catchCause(() =>
					Recover(
						query,
						current,
						"project_directory.unavailable",
						"The server directory listing could not be read.",
						true,
					),
				),
			),
		"project.directory.select": (query: ProjectDirectorySelectEnvelope, current: ReadyState) =>
			directories.Select(query.payload).pipe(
				Effect.flatMap(projects.Attach),
				Effect.flatMap((payload) =>
					Respond(query, { kind: "project.directory.select.result", payload }),
				),
				Effect.catchCause(() =>
					Recover(
						query,
						current,
						"project_directory.invalid",
						"The selected server directory is unavailable.",
						false,
					),
				),
			),
		"project.list.query": (query: ProjectListQueryEnvelope, current: ReadyState) =>
			projects.Snapshot.pipe(
				Effect.flatMap((payload) =>
					Respond(query, { kind: "project.list.query.result", payload }),
				),
				Effect.catchCause(() =>
					Recover(
						query,
						current,
						"project_catalog.unavailable",
						"The Forge project catalog could not be read.",
						true,
					),
				),
			),
		"project.repository.query": (query: ProjectRepositoryQueryEnvelope, current: ReadyState) =>
			projects.Snapshot.pipe(
				Effect.flatMap((catalog) => {
					const requested = new Set(query.payload.project_ids);
					const selected =
						requested.size === 0
							? catalog.projects
							: catalog.projects.filter((project) =>
									requested.has(project.project_id),
								);

					return Effect.forEach(
						selected,
						(project) =>
							repositories.Inspect(project.root_path).pipe(
								Effect.map((repository) => ({
									project_id: project.project_id,
									repository,
								})),
								Effect.catchCause(() =>
									Effect.succeed({
										project_id: project.project_id,
										repository: { state: literal("not_repository") },
									}),
								),
							),
						{ concurrency: 4 },
					);
				}),
				Effect.flatMap((repositories) =>
					Respond(query, {
						kind: "project.repository.query.result",
						payload: { repositories },
					}),
				),
				Effect.catchCause(() =>
					Recover(
						query,
						current,
						"project_catalog.unavailable",
						"The Forge project catalog could not be read.",
						true,
					),
				),
			),
		"project.diff.query": (query: ProjectDiffQueryEnvelope, current: ReadyState) =>
			projects.Snapshot.pipe(
				Effect.flatMap((catalog) => {
					const requested = new Set(query.payload.project_ids);
					const selected = (
						requested.size === 0
							? catalog.projects
							: catalog.projects.filter((project) =>
									requested.has(project.project_id),
								)
					).slice(0, ProjectDiffMaximumProjects);

					return Effect.forEach(
						selected,
						(project) =>
							repositories.Diff(project.root_path).pipe(
								Effect.map((diff) => ({ diff, project_id: project.project_id })),
								Effect.catchCause((cause) =>
									Effect.logWarning(
										`Project diff read failed for ${project.root_path}: ${String(cause)}`,
									).pipe(
										Effect.as({
											diff: { state: literal("not_repository") },
											project_id: project.project_id,
										}),
									),
								),
							),
						{ concurrency: 4 },
					);
				}),
				Effect.flatMap((diffs) =>
					Respond(query, { kind: "project.diff.query.result", payload: { diffs } }),
				),
				Effect.catchCause(() =>
					Recover(
						query,
						current,
						"project_catalog.unavailable",
						"The Forge project catalog could not be read.",
						true,
					),
				),
			),
		"session.defaults.query": (query: SessionDefaultsQueryEnvelope, current: ReadyState) =>
			defaults.Read.pipe(
				Effect.flatMap((payload) =>
					Respond(query, { kind: "session.defaults.query.result", payload }),
				),
				Effect.catchCause(() =>
					Recover(
						query,
						current,
						"session_defaults.unavailable",
						"The Forge session defaults could not be read.",
						true,
					),
				),
			),
		"project.detach": (query: ProjectDetachEnvelope, current: ReadyState) =>
			projects.Detach(query.payload.project_id).pipe(
				Effect.flatMap((payload) =>
					Respond(query, { kind: "project.detach.result", payload }),
				),
				Effect.catchCause(() =>
					Recover(
						query,
						current,
						"project_catalog.unavailable",
						"The Forge project could not be detached.",
						true,
					),
				),
			),
		"runtime.catalog.query": (query: RuntimeCatalogQueryEnvelope, current: ReadyState) =>
			runtime.Get.pipe(
				Effect.flatMap((payload) =>
					Respond(query, { kind: "runtime.catalog.query.result", payload }),
				),
				Effect.catchCause(() =>
					Recover(
						query,
						current,
						"runtime_catalog.unavailable",
						"The Forge runtime catalog could not be read.",
						true,
					),
				),
			),
	};

	return (query: ProjectQueryEnvelope, current: ReadyState): Effect.Effect<void> => {
		switch (query.kind) {
			case "project.directory.list.query":
				return handlers["project.directory.list.query"](query, current);
			case "project.directory.select":
				return handlers["project.directory.select"](query, current);
			case "project.list.query":
				return handlers["project.list.query"](query, current);
			case "project.repository.query":
				return handlers["project.repository.query"](query, current);
			case "project.diff.query":
				return handlers["project.diff.query"](query, current);
			case "session.defaults.query":
				return handlers["session.defaults.query"](query, current);
			case "project.detach":
				return handlers["project.detach"](query, current);
			case "runtime.catalog.query":
				return handlers["runtime.catalog.query"](query, current);
		}
	};
});

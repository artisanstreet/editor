import { and, asc, eq } from "drizzle-orm";
import { Context, Crypto, Data, Effect, Layer, Option, Schema } from "effect";

import { Database } from "../persistence/database";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import { ProjectHostedOrigins, Projects } from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import {
	HostedProjectIdentity,
	ProjectId,
	ProjectRoot,
	ProjectWorkspaceId,
	RegisteredProject,
	RegisterHostedProject,
	type ProjectHostedOrigin,
} from "./project";

export class ProjectRepositoryConflict extends Data.TaggedError("ProjectRepositoryConflict")<{
	readonly reason: "canonical_root" | "hosted_coordinate" | "workspace_id";
}> {}

export class ProjectRepositoryInvariant extends Data.TaggedError("ProjectRepositoryInvariant")<{
	readonly message: string;
}> {}

export class ProjectRepositoryInvalid extends Data.TaggedError("ProjectRepositoryInvalid")<{
	readonly message: string;
}> {}

export class ProjectRepositoryFailure extends Data.TaggedError("ProjectRepositoryFailure")<{
	readonly cause: unknown;
}> {}

export type ProjectRepositoryError =
	| ProjectRepositoryConflict
	| ProjectRepositoryFailure
	| ProjectRepositoryInvalid
	| ProjectRepositoryInvariant;

export interface ProjectRegistrationResult {
	readonly project: RegisteredProject;
	readonly status: "existing" | "registered";
}

/** Provides durable registration and lookup of one hosted project per checkout. */
export class ProjectRepository extends Context.Service<
	ProjectRepository,
	{
		readonly FindByHostedIdentity: (
			input: unknown,
		) => Effect.Effect<Option.Option<RegisteredProject>, ProjectRepositoryError>;
		readonly FindByProjectId: (
			input: unknown,
		) => Effect.Effect<Option.Option<RegisteredProject>, ProjectRepositoryError>;
		readonly FindByRoot: (
			input: unknown,
		) => Effect.Effect<Option.Option<RegisteredProject>, ProjectRepositoryError>;
		readonly FindByWorkspaceId: (
			input: unknown,
		) => Effect.Effect<Option.Option<RegisteredProject>, ProjectRepositoryError>;
		readonly List: Effect.Effect<ReadonlyArray<RegisteredProject>, ProjectRepositoryError>;
		readonly RegisterHosted: (
			input: unknown,
		) => Effect.Effect<ProjectRegistrationResult, ProjectRepositoryError>;
	}
>()("Artisan/ProjectRepository") {}

const text_encoder = new TextEncoder();

function decode_input<S extends Schema.ConstraintDecoder<unknown>>(schema: S, input: unknown) {
	const decoded = Schema.decodeUnknownOption(schema, { onExcessProperty: "error" })(input);

	return Effect.fromOption(decoded).pipe(
		Effect.mapError(
			() => new ProjectRepositoryInvalid({ message: "Hosted project input is invalid" }),
		),
	);
}

function hex(bytes: Uint8Array) {
	return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function frame_hosted_identity(origin: ProjectHostedOrigin) {
	const parts = [origin.provider_id, origin.canonical_host, origin.native_id];
	const buffers = parts.map((part) => text_encoder.encode(part));
	const size = buffers.reduce((total, buffer) => total + 4 + buffer.byteLength, 0);
	const framed = new Uint8Array(size);
	const view = new DataView(framed.buffer);
	let offset = 0;

	for (const buffer of buffers) {
		view.setUint32(offset, buffer.byteLength, false);
		offset += 4;
		framed.set(buffer, offset);
		offset += buffer.byteLength;
	}

	return framed;
}

function normalize_error(error: unknown): ProjectRepositoryError {
	if (
		error instanceof ProjectRepositoryConflict ||
		error instanceof ProjectRepositoryInvalid ||
		error instanceof ProjectRepositoryInvariant
	) {
		return error;
	}

	return new ProjectRepositoryFailure({ cause: error });
}

/** Supplies SQLite-backed hosted project registration with replay-safe identity semantics. */
export const ProjectRepositoryLive = Layer.effect(
	ProjectRepository,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const DeriveProjectIds = (origin: ProjectHostedOrigin) =>
			crypto.digest("SHA-256", frame_hosted_identity(origin)).pipe(
				Effect.map((digest) => {
					const identity_hash = hex(digest);

					return {
						project_id: `project_${identity_hash}`,
						workspace_id: `workspace_${identity_hash}`,
					};
				}),
			);

		const DecodeProject = (
			project_row: typeof Projects.$inferSelect,
			origin_row: typeof ProjectHostedOrigins.$inferSelect,
		) =>
			Effect.gen(function* () {
				const project = yield* Effect.fromOption(
					Schema.decodeUnknownOption(RegisteredProject, { onExcessProperty: "error" })({
						hosted_origin: {
							canonical_host: origin_row.canonical_host,
							clone_url: origin_row.clone_url,
							fetch_url: origin_row.fetch_url,
							name: origin_row.name,
							native_id: origin_row.native_id,
							owner: origin_row.owner,
							provider_id: origin_row.provider_id,
							push_url: origin_row.push_url,
							remote_name: origin_row.remote_name,
							selected_account_login: origin_row.selected_account_login,
							web_url: origin_row.web_url,
						},
						project: {
							display_name: project_row.display_name,
							project_id: project_row.project_id,
							root_path: project_row.canonical_root,
						},
						registered_at: project_row.registered_at,
						updated_at: project_row.updated_at,
						workspace_id: project_row.workspace_id,
					}),
				).pipe(
					Effect.mapError(
						() =>
							new ProjectRepositoryInvariant({
								message: `Stored project ${project_row.project_id} is malformed`,
							}),
					),
				);
				const expected_ids = yield* DeriveProjectIds(project.hosted_origin).pipe(
					Effect.mapError(normalize_error),
				);

				if (
					project.project.project_id !== expected_ids.project_id ||
					project.workspace_id !== expected_ids.workspace_id
				) {
					return yield* new ProjectRepositoryInvariant({
						message: `Stored project ${project_row.project_id} does not match its hosted identity`,
					});
				}

				return project;
			});

		const ReadProject = (project_row: typeof Projects.$inferSelect) =>
			Effect.gen(function* () {
				const origins = yield* database.client
					.select()
					.from(ProjectHostedOrigins)
					.where(eq(ProjectHostedOrigins.project_id, project_row.project_id));

				if (origins.length !== 1) {
					return yield* new ProjectRepositoryInvariant({
						message: `Project ${project_row.project_id} does not have exactly one hosted origin`,
					});
				}

				return yield* DecodeProject(project_row, origins[0]!);
			});

		const FindByHostedIdentity = (input: unknown) =>
			decode_input(HostedProjectIdentity, input).pipe(
				Effect.flatMap((identity) =>
					database.client
						.select()
						.from(ProjectHostedOrigins)
						.where(
							and(
								eq(ProjectHostedOrigins.provider_id, identity.provider_id),
								eq(ProjectHostedOrigins.canonical_host, identity.canonical_host),
								eq(ProjectHostedOrigins.native_id, identity.native_id),
							),
						)
						.pipe(
							Effect.flatMap((origins) => {
								if (origins.length === 0) return Effect.succeed(Option.none());
								if (origins.length > 1) {
									return Effect.fail(
										new ProjectRepositoryInvariant({
											message:
												"Hosted identity has multiple persisted origins",
										}),
									);
								}

								return database.client
									.select()
									.from(Projects)
									.where(eq(Projects.project_id, origins[0]!.project_id))
									.pipe(
										Effect.flatMap((projects) => {
											if (projects.length !== 1) {
												return Effect.fail(
													new ProjectRepositoryInvariant({
														message:
															"Hosted origin references a missing or duplicate project",
													}),
												);
											}

											return DecodeProject(projects[0]!, origins[0]!);
										}),
										Effect.map(Option.some),
									);
							}),
							Effect.mapError(normalize_error),
						),
				),
				Effect.mapError(normalize_error),
			);

		const FindByRoot = (input: unknown) =>
			decode_input(ProjectRoot, input)
				.pipe(
					Effect.flatMap(({ canonical_root }) =>
						database.client
							.select()
							.from(Projects)
							.where(eq(Projects.canonical_root, canonical_root))
							.pipe(
								Effect.flatMap((projects) => {
									if (projects.length === 0) return Effect.succeed(Option.none());
									if (projects.length > 1) {
										return Effect.fail(
											new ProjectRepositoryInvariant({
												message:
													"Canonical root has multiple persisted projects",
											}),
										);
									}

									return ReadProject(projects[0]!).pipe(Effect.map(Option.some));
								}),
							),
					),
					Effect.mapError(normalize_error),
				)
				.pipe(Effect.mapError(normalize_error));

		const FindByProjectId = (input: unknown) =>
			decode_input(ProjectId, input).pipe(
				Effect.flatMap(({ project_id }) =>
					database.client
						.select()
						.from(Projects)
						.where(eq(Projects.project_id, project_id))
						.pipe(
							Effect.flatMap((projects) => {
								if (projects.length === 0) return Effect.succeed(Option.none());
								if (projects.length > 1) {
									return Effect.fail(
										new ProjectRepositoryInvariant({
											message: "Project ID has multiple persisted projects",
										}),
									);
								}

								return ReadProject(projects[0]!).pipe(Effect.map(Option.some));
							}),
						),
				),
				Effect.mapError(normalize_error),
			);

		const FindByWorkspaceId = (input: unknown) =>
			decode_input(ProjectWorkspaceId, input)
				.pipe(
					Effect.flatMap(({ workspace_id }) =>
						database.client
							.select()
							.from(Projects)
							.where(eq(Projects.workspace_id, workspace_id))
							.pipe(
								Effect.flatMap((projects) => {
									if (projects.length === 0) return Effect.succeed(Option.none());
									if (projects.length > 1) {
										return Effect.fail(
											new ProjectRepositoryInvariant({
												message:
													"Workspace ID has multiple persisted projects",
											}),
										);
									}

									return ReadProject(projects[0]!).pipe(Effect.map(Option.some));
								}),
							),
					),
					Effect.mapError(normalize_error),
				)
				.pipe(Effect.mapError(normalize_error));

		const List = database.client
			.select()
			.from(Projects)
			.orderBy(asc(Projects.registered_at), asc(Projects.project_id))
			.pipe(
				Effect.flatMap((projects) => Effect.forEach(projects, ReadProject)),
				Effect.mapError(normalize_error),
			);

		const RegisterHosted = (input: unknown) =>
			decode_input(RegisterHostedProject, input).pipe(
				Effect.flatMap((registration) =>
					DeriveProjectIds(registration.hosted_origin).pipe(
						Effect.mapError(normalize_error),
						Effect.flatMap(({ project_id, workspace_id }) => {
							return RetrySqliteWrite(
								database.client.transaction((transaction) =>
									Effect.gen(function* () {
										const now = yield* metadata.Now;
										const origin = registration.hosted_origin;
										const native_origins = yield* transaction
											.select()
											.from(ProjectHostedOrigins)
											.where(
												and(
													eq(
														ProjectHostedOrigins.provider_id,
														origin.provider_id,
													),
													eq(
														ProjectHostedOrigins.canonical_host,
														origin.canonical_host,
													),
													eq(
														ProjectHostedOrigins.native_id,
														origin.native_id,
													),
												),
											);
										const coordinate_origins = yield* transaction
											.select()
											.from(ProjectHostedOrigins)
											.where(
												and(
													eq(
														ProjectHostedOrigins.provider_id,
														origin.provider_id,
													),
													eq(
														ProjectHostedOrigins.canonical_host,
														origin.canonical_host,
													),
													eq(ProjectHostedOrigins.owner, origin.owner),
													eq(ProjectHostedOrigins.name, origin.name),
												),
											);
										const root_projects = yield* transaction
											.select()
											.from(Projects)
											.where(
												eq(
													Projects.canonical_root,
													registration.canonical_root,
												),
											);
										const workspace_projects = yield* transaction
											.select()
											.from(Projects)
											.where(eq(Projects.workspace_id, workspace_id));

										if (
											native_origins.length > 1 ||
											coordinate_origins.length > 1
										) {
											return yield* new ProjectRepositoryInvariant({
												message: "Hosted identity uniqueness is corrupt",
											});
										}

										if (native_origins.length === 1) {
											const existing_origin = native_origins[0]!;

											if (
												coordinate_origins.length === 1 &&
												coordinate_origins[0]!.project_id !==
													existing_origin.project_id
											) {
												return yield* new ProjectRepositoryConflict({
													reason: "hosted_coordinate",
												});
											}

											const projects = yield* transaction
												.select()
												.from(Projects)
												.where(
													eq(
														Projects.project_id,
														existing_origin.project_id,
													),
												);

											if (projects.length !== 1) {
												return yield* new ProjectRepositoryInvariant({
													message:
														"Hosted origin references a missing or duplicate project",
												});
											}

											return {
												project: yield* DecodeProject(
													projects[0]!,
													existing_origin,
												),
												status: "existing" as const,
											};
										}

										if (coordinate_origins.length === 1) {
											return yield* new ProjectRepositoryConflict({
												reason: "hosted_coordinate",
											});
										}

										if (
											root_projects.length > 1 ||
											workspace_projects.length > 1
										) {
											return yield* new ProjectRepositoryInvariant({
												message:
													"Project root or workspace uniqueness is corrupt",
											});
										}

										if (root_projects.length === 1) {
											return yield* new ProjectRepositoryConflict({
												reason: "canonical_root",
											});
										}

										if (workspace_projects.length === 1) {
											return yield* new ProjectRepositoryConflict({
												reason: "workspace_id",
											});
										}

										yield* transaction.insert(Projects).values({
											canonical_root: registration.canonical_root,
											display_name: registration.display_name,
											project_id,
											registered_at: now,
											updated_at: now,
											workspace_id,
										});
										yield* transaction.insert(ProjectHostedOrigins).values({
											canonical_host: origin.canonical_host,
											clone_url: origin.clone_url,
											fetch_url: origin.fetch_url,
											name: origin.name,
											native_id: origin.native_id,
											owner: origin.owner,
											project_id,
											provider_id: origin.provider_id,
											push_url: origin.push_url,
											remote_name: origin.remote_name,
											selected_account_login: origin.selected_account_login,
											web_url: origin.web_url,
										});

										const project = yield* Effect.fromOption(
											Schema.decodeUnknownOption(RegisteredProject)({
												hosted_origin: origin,
												project: {
													display_name: registration.display_name,
													project_id,
													root_path: registration.canonical_root,
												},
												registered_at: now,
												updated_at: now,
												workspace_id,
											}),
										).pipe(
											Effect.mapError(
												() =>
													new ProjectRepositoryInvariant({
														message:
															"Derived hosted project identifiers are invalid",
													}),
											),
											Effect.map((value): RegisteredProject => value),
										);

										return {
											project,
											status: "registered" as const,
										};
									}),
								),
							).pipe(Effect.mapError(normalize_error));
						}),
					),
				),
			);

		return {
			FindByHostedIdentity,
			FindByProjectId,
			FindByRoot,
			FindByWorkspaceId,
			List,
			RegisterHosted,
		} satisfies typeof ProjectRepository.Service;
	}),
);

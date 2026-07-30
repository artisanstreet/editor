import { Effect } from "effect";

import {
	type ArtisanApprovalListQueryEnvelope,
	type ArtisanToolInvocationListQueryEnvelope,
	type ArtisanToolRegistryListQueryEnvelope,
	type EngineUsageQuery,
	type EngineUsageQueryEnvelope,
	type HostIdentityQueryEnvelope,
	type ProjectDetachEnvelope,
	type ProjectDiffQueryEnvelope,
	type ProjectDirectoryListInput,
	type ProjectDirectoryListQueryEnvelope,
	type ProjectDirectorySelectEnvelope,
	type ProjectDirectorySelectInput,
	type ProjectListQueryEnvelope,
	type ProjectRepositoryQueryEnvelope,
	type RuntimeCatalogQueryEnvelope,
	type ThreadCreateEnvelope,
	type ThreadCreateInput,
	type ThreadListQueryEnvelope,
} from "@artisan/protocol";

import type {
	ArtisanApprovalListInput,
	ArtisanToolInvocationListInput,
	ArtisanToolRegistryListInput,
} from "../../client-api/service";
import { ClientApiContext } from "./context";

/** Constructs thread, project, runtime, and tool read operations. */
export const MakeQueryApi = Effect.gen(function* () {
	const context = yield* ClientApiContext;
	const list_threads = Effect.gen(function* () {
		const trace = yield* context.MakeTrace;
		const result = yield* context.Request({
			...trace,
			kind: "thread.list.query",
			payload: {},
		} satisfies ThreadListQueryEnvelope);
		return result.kind === "thread.list.query.result"
			? result.payload.threads
			: yield* Effect.die("thread list response narrowed incorrectly");
	});
	const create_thread = (input: ThreadCreateInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "thread.create.request",
				payload: input,
			} satisfies ThreadCreateEnvelope);
			return result.kind === "thread.create.result"
				? result.payload
				: yield* Effect.die("thread create response narrowed incorrectly");
		});
	const list_project_directories = (input: ProjectDirectoryListInput = {}) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "project.directory.list.query",
				payload: input,
			} satisfies ProjectDirectoryListQueryEnvelope);
			return result.kind === "project.directory.list.query.result"
				? result.payload
				: yield* Effect.die("project directory list response narrowed incorrectly");
		});
	const select_project_directory = (input: ProjectDirectorySelectInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "project.directory.select",
				payload: input,
			} satisfies ProjectDirectorySelectEnvelope);
			return result.kind === "project.directory.select.result"
				? result.payload
				: yield* Effect.die("project directory select response narrowed incorrectly");
		});
	const list_projects = Effect.gen(function* () {
		const trace = yield* context.MakeTrace;
		const result = yield* context.Request({
			...trace,
			kind: "project.list.query",
			payload: {},
		} satisfies ProjectListQueryEnvelope);
		return result.kind === "project.list.query.result"
			? result.payload
			: yield* Effect.die("project list response narrowed incorrectly");
	});
	const get_runtime_catalog = Effect.gen(function* () {
		const trace = yield* context.MakeTrace;
		const result = yield* context.Request({
			...trace,
			kind: "runtime.catalog.query",
			payload: {},
		} satisfies RuntimeCatalogQueryEnvelope);
		return result.kind === "runtime.catalog.query.result"
			? result.payload
			: yield* Effect.die("runtime catalog response narrowed incorrectly");
	});
	const get_host_identity = Effect.gen(function* () {
		const trace = yield* context.MakeTrace;
		const result = yield* context.Request({
			...trace,
			kind: "host.identity.query",
			payload: {},
		} satisfies HostIdentityQueryEnvelope);
		return result.kind === "host.identity.query.result"
			? result.payload
			: yield* Effect.die("host identity response narrowed incorrectly");
	});
	const get_project_repositories = (project_ids: ReadonlyArray<string> = []) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "project.repository.query",
				payload: { project_ids },
			} satisfies ProjectRepositoryQueryEnvelope);
			return result.kind === "project.repository.query.result"
				? result.payload
				: yield* Effect.die("project repository response narrowed incorrectly");
		});
	const get_project_diffs = (project_ids: ReadonlyArray<string> = []) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "project.diff.query",
				payload: { project_ids },
			} satisfies ProjectDiffQueryEnvelope);
			return result.kind === "project.diff.query.result"
				? result.payload
				: yield* Effect.die("project diff response narrowed incorrectly");
		});
	const get_engine_usage = (input?: EngineUsageQuery) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "engine.usage.query",
				payload: input ?? {},
			} satisfies EngineUsageQueryEnvelope);
			return result.kind === "engine.usage.query.result"
				? result.payload
				: yield* Effect.die("engine usage response narrowed incorrectly");
		});
	const detach_project = (project_id: string) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "project.detach",
				payload: { project_id },
			} satisfies ProjectDetachEnvelope);
			return result.kind === "project.detach.result"
				? result.payload
				: yield* Effect.die("project detach response narrowed incorrectly");
		});
	const list_artisan_tools = (input: ArtisanToolRegistryListInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "artisan.tool.registry.list.query",
				payload: input,
			} satisfies ArtisanToolRegistryListQueryEnvelope);
			return result.kind === "artisan.tool.registry.list.query.result"
				? result.payload
				: yield* Effect.die("Artisan tool registry response narrowed incorrectly");
		});
	const list_artisan_tool_invocations = (input: ArtisanToolInvocationListInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "artisan.tool.invocation.list.query",
				payload: input,
			} satisfies ArtisanToolInvocationListQueryEnvelope);
			return result.kind === "artisan.tool.invocation.list.query.result"
				? result.payload
				: yield* Effect.die("Artisan tool invocation response narrowed incorrectly");
		});
	const list_artisan_approvals = (input: ArtisanApprovalListInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				kind: "artisan.approval.list.query",
				payload: input,
			} satisfies ArtisanApprovalListQueryEnvelope);
			return result.kind === "artisan.approval.list.query.result"
				? result.payload
				: yield* Effect.die("Artisan approval response narrowed incorrectly");
		});

	return {
		create_thread,
		detach_project,
		get_engine_usage,
		get_host_identity,
		get_project_diffs,
		get_project_repositories,
		get_runtime_catalog,
		list_artisan_approvals,
		list_artisan_tool_invocations,
		list_artisan_tools,
		list_project_directories,
		list_projects,
		list_threads,
		select_project_directory,
	};
});

import { Effect, Schedule } from "effect";

import {
  type ArtisanApprovalListQueryEnvelope,
  type ArtisanToolInvocationListQueryEnvelope,
  type ArtisanToolRegistryListQueryEnvelope,
  type ThreadUsageSeriesQuery,
  type ThreadUsageSeriesQueryEnvelope,
  type EngineUsageQuery,
  type EngineUsageQueryEnvelope,
  type EngineInstallationQuery,
  type EngineInstallationQueryEnvelope,
  type EngineInstallRequest,
  type EngineInstallRequestEnvelope,
  type EngineAuthenticationRequest,
  type EngineAuthenticationRequestEnvelope,
  type EngineRollbackRequest,
  type EngineRollbackRequestEnvelope,
  type HostIdentityQueryEnvelope,
  type HostMachineConnectRequest,
  type HostMachineConnectRequestEnvelope,
  type HostMachinesQueryEnvelope,
  type ProjectDetachEnvelope,
  type ProjectDiffQueryEnvelope,
  type ProjectIdentityQueryEnvelope,
  type ProjectDirectoryListInput,
  type ProjectDirectoryListQueryEnvelope,
  type ProjectDirectoryPickEnvelope,
  type ProjectDirectoryCreateEnvelope,
  type ProjectDirectoryCreateInput,
  type ProjectDirectorySelectEnvelope,
  type ProjectDirectorySelectInput,
  type ProjectListQueryEnvelope,
  type ProjectRepositoryQueryEnvelope,
  type RuntimeCatalogQueryEnvelope,
  type ThreadCreateEnvelope,
  type ThreadCreateInput,
  type ThreadListQueryEnvelope,
  type TelemetryIntent,
  type TelemetryIntentCaptureEnvelope,
  type TelemetryPreferencesQueryEnvelope,
  type TelemetryPreferencesUpdate,
  type TelemetryPreferencesUpdateEnvelope,
} from "@artisan/protocol";

import type {
  ArtisanApprovalListInput,
  ArtisanClientError,
  ArtisanToolInvocationListInput,
  ArtisanToolRegistryListInput,
} from "../../client-api/service";
import { ClientApiContext } from "./context";

/**
 * Bounds the one read the suite renders for every engine at once.
 *
 * The engine menu lays out Codex, Claude, Grok and Cursor from a *single*
 * `engine.usage.query` — the backend already isolates per-engine failure
 * behind its own 15s timeout and a last-good cache, so one provider being
 * slow or broken costs that provider's row and nothing else. What it cannot
 * absorb is losing the reply itself: an unanswered request has no per-engine
 * detail to fall back to, so all four rows render the same deadline text and
 * a wedged local broker reads as every provider going down at once.
 *
 * Retrying is safe here in a way it is not for a command. This is a read, and
 * the backend coalesces concurrent probes for the same engine onto one
 * in-flight run, so a second attempt neither re-spawns a provider CLI that is
 * still running nor manufactures a fresh answer — it usually returns the
 * reports the abandoned attempt had already gathered and cached.
 *
 * Kept short and few: a dropped connection fails fast and recovers within a
 * second or so, and a miss that survives three attempts is a broker that is
 * genuinely not answering, which the caller should see rather than sit behind
 * a spinner for.
 */
const engine_usage_retry_schedule = Schedule.exponential("300 millis").pipe(
  Schedule.jittered,
  Schedule.upTo({ times: 2 }),
);

/**
 * Retries only the failures that mean nothing answered — a deadline miss or a
 * dropped connection. A protocol rejection is the backend having decided, and
 * a decision stands; repeating it just asks the same question twice.
 */
const engine_usage_failure_is_transient = (error: ArtisanClientError) =>
  error.retryable && error.code === "connection";

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
  const get_telemetry_preferences = Effect.gen(function* () {
    const trace = yield* context.MakeTrace;
    const result = yield* context.Request({
      ...trace,
      kind: "telemetry.preferences.query",
      payload: {},
    } satisfies TelemetryPreferencesQueryEnvelope);
    return result.kind === "telemetry.preferences.query.result"
      ? result.payload
      : yield* Effect.die("telemetry preferences response narrowed incorrectly");
  });
  const update_telemetry_preferences = (input: TelemetryPreferencesUpdate) =>
    Effect.gen(function* () {
      const trace = yield* context.MakeTrace;
      const result = yield* context.Request({
        ...trace,
        kind: "telemetry.preferences.update",
        payload: input,
      } satisfies TelemetryPreferencesUpdateEnvelope);
      return result.kind === "telemetry.preferences.update.result"
        ? result.payload
        : yield* Effect.die("telemetry preference update response narrowed incorrectly");
    });
  const capture_telemetry_intent = (input: TelemetryIntent) =>
    Effect.gen(function* () {
      const trace = yield* context.MakeTrace;
      const result = yield* context.Request({
        ...trace,
        kind: "telemetry.intent.capture",
        payload: input,
      } satisfies TelemetryIntentCaptureEnvelope);
      if (result.kind !== "telemetry.intent.capture.result") {
        return yield* Effect.die("telemetry capture response narrowed incorrectly");
      }
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
  const pick_project_directory = Effect.gen(function* () {
    const trace = yield* context.MakeTrace;
    const result = yield* context.Request({
      ...trace,
      kind: "project.directory.pick",
      payload: {},
    } satisfies ProjectDirectoryPickEnvelope);
    return result.kind === "project.directory.pick.result"
      ? result.payload
      : yield* Effect.die("project directory pick response narrowed incorrectly");
  });
  const create_project_directory = (input: ProjectDirectoryCreateInput) =>
    Effect.gen(function* () {
      const trace = yield* context.MakeTrace;
      const result = yield* context.Request({
        ...trace,
        kind: "project.directory.create",
        payload: input,
      } satisfies ProjectDirectoryCreateEnvelope);
      return result.kind === "project.directory.create.result"
        ? result.payload
        : yield* Effect.die("project directory create response narrowed incorrectly");
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
  const get_host_machines = Effect.gen(function* () {
    const trace = yield* context.MakeTrace;
    const result = yield* context.Request({
      ...trace,
      kind: "host.machines.query",
      payload: {},
    } satisfies HostMachinesQueryEnvelope);
    return result.kind === "host.machines.query.result"
      ? result.payload
      : yield* Effect.die("host machines response narrowed incorrectly");
  });
  const connect_host_machine = (input: HostMachineConnectRequest) =>
    Effect.gen(function* () {
      const trace = yield* context.MakeTrace;
      const result = yield* context.Request({
        ...trace,
        kind: "host.machines.connect.request",
        payload: input,
      } satisfies HostMachineConnectRequestEnvelope);
      return result.kind === "host.machines.connect.result"
        ? result.payload
        : yield* Effect.die("host machine connect response narrowed incorrectly");
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
  const get_project_identities = (project_ids: ReadonlyArray<string> = []) =>
    Effect.gen(function* () {
      const trace = yield* context.MakeTrace;
      const result = yield* context.Request({
        ...trace,
        kind: "project.identity.query",
        payload: { project_ids },
      } satisfies ProjectIdentityQueryEnvelope);
      return result.kind === "project.identity.query.result"
        ? result.payload
        : yield* Effect.die("project identity response narrowed incorrectly");
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
      /**
       * Inside the retried region on purpose: every attempt has to carry a
       * fresh `message_id`. The request coordinator refuses an id it already
       * holds or has tombstoned, so a schedule that resent one completed
       * envelope would turn each retry into a correlation conflict instead
       * of a second ask.
       */
      const trace = yield* context.MakeTrace;
      const result = yield* context.Request({
        ...trace,
        kind: "engine.usage.query",
        payload: input ?? {},
      } satisfies EngineUsageQueryEnvelope);
      return result.kind === "engine.usage.query.result"
        ? result.payload
        : yield* Effect.die("engine usage response narrowed incorrectly");
    }).pipe(
      Effect.retry({
        schedule: engine_usage_retry_schedule,
        while: engine_usage_failure_is_transient,
      }),
    );
  const get_engine_installations = (input?: EngineInstallationQuery) =>
    Effect.gen(function* () {
      const trace = yield* context.MakeTrace;
      const result = yield* context.Request({
        ...trace,
        kind: "engine.installation.query",
        payload: input ?? {},
      } satisfies EngineInstallationQueryEnvelope);
      return result.kind === "engine.installation.query.result"
        ? result.payload
        : yield* Effect.die("engine installation response narrowed incorrectly");
    });
  const install_engine = (input: EngineInstallRequest) =>
    Effect.gen(function* () {
      const trace = yield* context.MakeTrace;
      const result = yield* context.Request({
        ...trace,
        kind: "engine.install.request",
        payload: input,
      } satisfies EngineInstallRequestEnvelope);
      return result.kind === "engine.installation.mutation.result"
        ? result.payload
        : yield* Effect.die("engine install response narrowed incorrectly");
    });
  const authenticate_engine = (input: EngineAuthenticationRequest) =>
    Effect.gen(function* () {
      const trace = yield* context.MakeTrace;
      const result = yield* context.Request({
        ...trace,
        kind: "engine.authentication.request",
        payload: input,
      } satisfies EngineAuthenticationRequestEnvelope);
      return result.kind === "engine.installation.mutation.result"
        ? result.payload
        : yield* Effect.die("engine authentication response narrowed incorrectly");
    });
  const rollback_engine = (input: EngineRollbackRequest) =>
    Effect.gen(function* () {
      const trace = yield* context.MakeTrace;
      const result = yield* context.Request({
        ...trace,
        kind: "engine.rollback.request",
        payload: input,
      } satisfies EngineRollbackRequestEnvelope);
      return result.kind === "engine.installation.mutation.result"
        ? result.payload
        : yield* Effect.die("engine rollback response narrowed incorrectly");
    });
  const get_thread_usage_series = (input: ThreadUsageSeriesQuery) =>
    Effect.gen(function* () {
      const trace = yield* context.MakeTrace;
      const result = yield* context.Request({
        ...trace,
        kind: "thread.usage.series.query",
        payload: input,
      } satisfies ThreadUsageSeriesQueryEnvelope);
      return result.kind === "thread.usage.series.query.result"
        ? result.payload
        : yield* Effect.die("thread usage series response narrowed incorrectly");
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
    capture_telemetry_intent,
    create_project_directory,
    create_thread,
    authenticate_engine,
    detach_project,
    get_thread_usage_series,
    get_engine_usage,
    get_engine_installations,
    connect_host_machine,
    get_host_identity,
    get_host_machines,
    get_project_diffs,
    get_project_identities,
    get_project_repositories,
    get_runtime_catalog,
    get_telemetry_preferences,
    list_artisan_approvals,
    list_artisan_tool_invocations,
    list_artisan_tools,
    list_project_directories,
    list_projects,
    list_threads,
    install_engine,
    pick_project_directory,
    rollback_engine,
    select_project_directory,
    update_telemetry_preferences,
  };
});

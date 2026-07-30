import { Clock, Context, Data, Effect, Layer, Option, Ref, Scope } from "effect";
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process";

import { is_local_preview_hostname } from "./network-policy";
import { PreviewTarget, type PreviewHealthProbeResult, type PreviewTargetRecord } from "./target";

/** Identifies the actor and connector that explicitly opened an inspection. */
export interface PreviewInspectionOpen {
	readonly actor_id: string;
	readonly connector_id: string;
	readonly target_id: string;
}

/** Represents one attributable, explicitly opened inspection session. */
export interface PreviewInspectionSession extends PreviewInspectionOpen {
	readonly opened_at_ms: number;
	readonly session_id: string;
}

/** Reports a rejected preview runtime operation without leaking target internals. */
export class PreviewRuntimeError extends Data.TaggedError("PreviewRuntimeError")<{
	readonly cause: unknown;
	readonly code:
		| "browser_unavailable"
		| "connector_failed"
		| "connector_unavailable"
		| "invalid_input"
		| "not_found";
	readonly target_id: string;
}> {}

/** Identifies the immutable attribution and target passed to one external connector open. */
export interface PreviewInspectionConnectorOpen extends PreviewInspectionOpen {
	readonly target: PreviewTargetRecord;
}

declare const PreviewInspectionConnectorHandleTypeId: unique symbol;

/**
 * An opaque connector-owned session handle. It deliberately has no browser or page
 * surface: only the connector may inspect or close it.
 */
export interface PreviewInspectionConnectorHandle {
	readonly [PreviewInspectionConnectorHandleTypeId]: typeof PreviewInspectionConnectorHandleTypeId;
}

/** Reports an unavailable or failed explicit external-browser connector action. */
export class PreviewInspectionConnectorError extends Data.TaggedError(
	"PreviewInspectionConnectorError",
)<{
	readonly cause: unknown;
	readonly code: "failed" | "unavailable";
	readonly target_id: string;
}> {}

/**
 * Connects an explicitly attributable inspection session to an external browser.
 * This is intentionally narrower than a browser registry: it cannot render pages,
 * discover browsers, or automate targets that were not explicitly opened here.
 */
export class PreviewInspectionConnector extends Context.Service<
	PreviewInspectionConnector,
	{
		readonly Close: (
			handle: PreviewInspectionConnectorHandle,
		) => Effect.Effect<void, PreviewInspectionConnectorError>;
		readonly Inspect: (
			handle: PreviewInspectionConnectorHandle,
		) => Effect.Effect<PreviewHealthProbeResult, PreviewInspectionConnectorError>;
		readonly Open: (
			input: PreviewInspectionConnectorOpen,
		) => Effect.Effect<PreviewInspectionConnectorHandle, PreviewInspectionConnectorError>;
	}
>()("Artisan/PreviewInspectionConnector") {}

/**
 * Fails closed until a separately capability-gated external-browser connector is
 * installed. The production preview runtime therefore never silently substitutes
 * direct target probing or an embedded browser for an inspection connector.
 */
export const PreviewInspectionConnectorUnavailableLive = Layer.succeed(PreviewInspectionConnector, {
	Close: () => Effect.void,
	Inspect: () =>
		Effect.fail(
			new PreviewInspectionConnectorError({
				cause: new Error("external browser inspection connector is unavailable"),
				code: "unavailable",
				target_id: "",
			}),
		),
	Open: (input) =>
		Effect.fail(
			new PreviewInspectionConnectorError({
				cause: new Error("external browser inspection connector is unavailable"),
				code: "unavailable",
				target_id: input.target_id,
			}),
		),
});

/** Opens a registered loopback preview in the user's configured external browser. */
export class PreviewExternalBrowser extends Context.Service<
	PreviewExternalBrowser,
	{
		readonly Launch: (
			input: Pick<PreviewInspectionOpen, "actor_id" | "target_id">,
		) => Effect.Effect<void, PreviewRuntimeError, Scope.Scope>;
	}
>()("Artisan/PreviewExternalBrowser") {}

/** Owns attributable inspection sessions; it never embeds, launches, or restarts a browser. */
export class PreviewInspection extends Context.Service<
	PreviewInspection,
	{
		readonly Close: (session_id: string) => Effect.Effect<void, PreviewRuntimeError>;
		readonly Inspect: (
			session_id: string,
		) => Effect.Effect<PreviewHealthProbeResult, PreviewRuntimeError, Scope.Scope>;
		readonly List: Effect.Effect<ReadonlyArray<PreviewInspectionSession>>;
		readonly Open: (
			input: PreviewInspectionOpen,
		) => Effect.Effect<PreviewInspectionSession, PreviewRuntimeError>;
	}
>()("Artisan/PreviewInspection") {}

function runtime_error(target_id: string, code: PreviewRuntimeError["code"], cause: unknown) {
	return new PreviewRuntimeError({ cause, code, target_id });
}

function is_nonempty(value: string) {
	return value.trim().length > 0;
}

function require_attribution(input: PreviewInspectionOpen) {
	return (
		is_nonempty(input.actor_id) &&
		is_nonempty(input.connector_id) &&
		is_nonempty(input.target_id)
	);
}

/** Builds the explicit inspection-session owner with scope-finalized cleanup. */
export const make_preview_inspection_layer = () =>
	Layer.effect(
		PreviewInspection,
		Effect.gen(function* () {
			const targets = yield* PreviewTarget;
			const connector = yield* PreviewInspectionConnector;
			const sessions = yield* Ref.make(
				new Map<
					string,
					{
						readonly handle: PreviewInspectionConnectorHandle;
						readonly session: PreviewInspectionSession;
					}
				>(),
			);
			let next_session_id = 0;
			const Close = (session_id: string) =>
				Effect.gen(function* () {
					const removed = yield* Ref.modify(sessions, (current) => {
						const live = current.get(session_id);

						if (live === undefined) {
							return [undefined, current] as const;
						}

						const next = new Map(current);

						next.delete(session_id);

						return [live, next] as const;
					});

					if (removed !== undefined) {
						yield* connector
							.Close(removed.handle)
							.pipe(
								Effect.mapError((cause) =>
									runtime_error(
										removed.session.target_id,
										"connector_failed",
										cause,
									),
								),
							);
					}
				});
			const required_session = (session_id: string) =>
				Ref.get(sessions).pipe(
					Effect.flatMap((current) => {
						const session = current.get(session_id);

						return session
							? Effect.succeed(session)
							: Effect.fail(
									runtime_error(
										"",
										"not_found",
										new Error("preview inspection session not found"),
									),
								);
					}),
				);

			const CloseAll = Ref.modify(
				sessions,
				(current) => [[...current.values()], new Map()] as const,
			).pipe(
				Effect.flatMap((live) =>
					Effect.forEach(
						live,
						({ handle }) => connector.Close(handle).pipe(Effect.ignore),
						{ discard: true },
					),
				),
			);

			yield* Effect.addFinalizer(() => CloseAll);

			return {
				Close,
				Inspect: (session_id) =>
					Effect.gen(function* () {
						const live = yield* required_session(session_id);

						return yield* connector
							.Inspect(live.handle)
							.pipe(
								Effect.mapError((cause) =>
									runtime_error(
										live.session.target_id,
										cause.code === "unavailable"
											? "connector_unavailable"
											: "connector_failed",
										cause,
									),
								),
							);
					}),
				List: Ref.get(sessions).pipe(
					Effect.map((current) =>
						[...current.values()]
							.map(({ session }) => session)
							.toSorted((left, right) =>
								left.session_id.localeCompare(right.session_id),
							),
					),
				),
				Open: (input) =>
					Effect.gen(function* () {
						if (!require_attribution(input)) {
							return yield* Effect.fail(
								runtime_error(
									input.target_id,
									"invalid_input",
									new Error("inspection attribution must be nonempty"),
								),
							);
						}

						const target = yield* targets.Get(input.target_id);

						if (Option.isNone(target)) {
							return yield* Effect.fail(
								runtime_error(
									input.target_id,
									"not_found",
									new Error("preview target not found"),
								),
							);
						}

						const session: PreviewInspectionSession = {
							...input,
							opened_at_ms: yield* Clock.currentTimeMillis,
							session_id: `preview-inspection-${++next_session_id}`,
						};

						const handle = yield* connector
							.Open({ ...input, target: target.value })
							.pipe(
								Effect.mapError((cause) =>
									runtime_error(
										input.target_id,
										cause.code === "unavailable"
											? "connector_unavailable"
											: "connector_failed",
										cause,
									),
								),
							);

						yield* Ref.update(sessions, (current) =>
							new Map(current).set(session.session_id, { handle, session }),
						);
						return session;
					}),
			};
		}),
	);

/** Describes the shell-free OS opener executable and fixed arguments for one platform. */
export interface PreviewBrowserOpener {
	readonly args: ReadonlyArray<string>;
	readonly command: string;
}

/**
 * Returns the configured OS URL handler command. Effect supplies scoped child-process
 * management but has no cross-platform external-browser opener, so selecting the
 * platform handler is deliberately this small Node boundary.
 */
export function preview_browser_opener(
	platform: NodeJS.Platform,
	url: string,
): PreviewBrowserOpener | undefined {
	if (platform === "win32") {
		return { args: ["url.dll,FileProtocolHandler", url], command: "rundll32.exe" };
	}

	if (platform === "darwin") {
		return { args: [url], command: "open" };
	}

	if (platform === "linux") {
		return { args: [url], command: "xdg-open" };
	}

	return undefined;
}

/** Builds the explicit external-browser launcher over Effect's scoped ChildProcess service. */
export const make_preview_external_browser_layer = (platform: NodeJS.Platform) =>
	Layer.effect(
		PreviewExternalBrowser,
		Effect.gen(function* () {
			const targets = yield* PreviewTarget;
			const spawner = yield* ChildProcessSpawner.ChildProcessSpawner;

			return {
				Launch: (input) =>
					Effect.gen(function* () {
						if (!is_nonempty(input.actor_id) || !is_nonempty(input.target_id)) {
							return yield* Effect.fail(
								runtime_error(
									input.target_id,
									"invalid_input",
									new Error("launch attribution must be nonempty"),
								),
							);
						}

						const target = yield* targets.Get(input.target_id);

						if (Option.isNone(target)) {
							return yield* Effect.fail(
								runtime_error(
									input.target_id,
									"not_found",
									new Error("preview target not found"),
								),
							);
						}

						const opener = preview_browser_opener(platform, target.value.url);

						if (!opener) {
							return yield* Effect.fail(
								runtime_error(
									input.target_id,
									"browser_unavailable",
									new Error("no configured browser opener"),
								),
							);
						}

						const handle = yield* spawner
							.spawn(
								ChildProcess.make(opener.command, opener.args, {
									shell: false,
									stderr: "ignore",
									stdin: "ignore",
									stdout: "ignore",
								}),
							)
							.pipe(
								Effect.mapError((cause) =>
									runtime_error(input.target_id, "browser_unavailable", cause),
								),
							);
						const exit_code = yield* Effect.raceFirst(
							handle.exitCode,
							Effect.sleep("10 seconds").pipe(
								Effect.flatMap(() =>
									Effect.fail(
										runtime_error(
											input.target_id,
											"browser_unavailable",
											new Error("external browser opener timed out"),
										),
									),
								),
							),
						).pipe(
							Effect.mapError((cause) =>
								runtime_error(input.target_id, "browser_unavailable", cause),
							),
						);

						if (exit_code !== 0) {
							return yield* Effect.fail(
								runtime_error(
									input.target_id,
									"browser_unavailable",
									new Error("external browser opener exited unsuccessfully"),
								),
							);
						}
					}),
			};
		}),
	);

/** Returns true only for a credential-free loopback HTTP(S) target. */
export function is_valid_loopback_preview_url(value: string) {
	try {
		const url = new URL(value);

		return (
			(url.protocol === "http:" || url.protocol === "https:") &&
			!url.username &&
			!url.password &&
			is_local_preview_hostname(url.hostname)
		);
	} catch {
		return false;
	}
}

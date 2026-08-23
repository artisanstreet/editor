import { Buffer } from "node:buffer";
import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";

import {
	type EngineAccountUsage,
	type EngineFailure,
	type EngineQuotaWindow,
	EngineProtocolError,
	EngineUnavailableError,
} from "../engine";

const cursor_usage_endpoint =
	"https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";
const cursor_auth_file_maximum_bytes = 1_048_576;
const cursor_usage_timeout_ms = 10_000;

interface CursorUsageHttpResponse {
	readonly ok: boolean;
	readonly status: number;
	readonly json: () => Promise<unknown>;
}

/** The small HTTP seam used by Cursor usage tests; production supplies the platform fetch. */
export type CursorUsageFetch = (
	url: string,
	init: {
		readonly body: string;
		readonly headers: Readonly<Record<string, string>>;
		readonly method: "POST";
		readonly signal: AbortSignal;
	},
) => Promise<CursorUsageHttpResponse>;

/** Configures one authenticated, non-billable Cursor dashboard usage read. */
export interface CursorUsageOptions {
	readonly auth_file?: string;
	readonly environment?: NodeJS.ProcessEnv;
	readonly Fetch?: CursorUsageFetch;
	readonly home_directory?: string;
	readonly platform?: NodeJS.Platform;
	/** Test seam that avoids reading a real credential file. */
	readonly ReadAccessToken?: () => Promise<string | undefined>;
	readonly timeout_ms?: number;
}

const as_record = (value: unknown, name: string): Readonly<Record<string, unknown>> => {
	if (typeof value !== "object" || value === null || Array.isArray(value))
		throw new Error(`${name} is not an object`);
	return value as Readonly<Record<string, unknown>>;
};

const optional_record = (
	value: unknown,
	name: string,
): Readonly<Record<string, unknown>> | undefined =>
	value === undefined ? undefined : as_record(value, name);

const optional_number = (
	record: Readonly<Record<string, unknown>>,
	key: string,
): number | undefined => {
	const value = record[key];
	if (value === undefined || value === null) return undefined;
	const number =
		typeof value === "number" ? value : typeof value === "string" ? Number(value) : NaN;
	if (!Number.isFinite(number)) throw new Error(`${key} is not numeric`);
	return number;
};

const clamp_percent = (value: number) => Math.min(100, Math.max(0, value));

const iso_date = (milliseconds: number | undefined): string | undefined => {
	if (milliseconds === undefined) return undefined;
	const date = new Date(milliseconds);
	return Number.isFinite(date.getTime()) ? date.toISOString() : undefined;
};

const period_minutes = (
	start_ms: number | undefined,
	end_ms: number | undefined,
): number | undefined => {
	if (start_ms === undefined || end_ms === undefined || end_ms <= start_ms) return undefined;
	const minutes = Math.round((end_ms - start_ms) / 60_000);
	return minutes > 0 ? minutes : undefined;
};

const percentage_from_display_message = (message: unknown): number | undefined => {
	if (typeof message !== "string") return undefined;
	const matched = /\b(\d+(?:\.\d+)?)%\b/.exec(message)?.[1];
	return matched === undefined ? undefined : Number(matched);
};

/**
 * Maps Cursor's DashboardService response to the provider-neutral meters.
 * Protobuf JSON omits numeric zeroes, so absent numeric fields are treated as
 * zero only after the containing usage object has been validated.
 */
export function map_cursor_period_usage_to_quota_windows(
	input: unknown,
): ReadonlyArray<EngineQuotaWindow> {
	const response = as_record(input, "Cursor usage response");
	const plan = optional_record(response.planUsage, "planUsage");
	if (plan === undefined) throw new Error("planUsage is missing");

	const start_ms = optional_number(response, "billingCycleStart");
	const end_ms = optional_number(response, "billingCycleEnd");
	const resets_at = iso_date(end_ms);
	const window_minutes = period_minutes(start_ms, end_ms);
	const total_spend = optional_number(plan, "totalSpend") ?? 0;
	const included_limit = optional_number(plan, "limit") ?? 0;
	const provider_percent = optional_number(plan, "totalPercentUsed");
	const plan_percent =
		included_limit > 0
			? (total_spend / included_limit) * 100
			: (provider_percent ?? percentage_from_display_message(response.displayMessage) ?? 0);
	const cursor_models_percent = optional_number(plan, "autoPercentUsed");
	const other_models_percent = optional_number(plan, "apiPercentUsed");
	/**
	 * Cursor's current dashboard reports two independent plan pools. Repeated
	 * protobuf fields survive at zero more reliably than scalar percentages, so
	 * `autoBucketModels` is also the compatibility discriminator when an unused
	 * pool's zero-valued percentage is omitted from protobuf JSON.
	 */
	const has_split_plan_usage =
		cursor_models_percent !== undefined ||
		other_models_percent !== undefined ||
		Array.isArray(response.autoBucketModels);
	const plan_windows: ReadonlyArray<Pick<EngineQuotaWindow, "id" | "label" | "percent_used">> =
		has_split_plan_usage
			? [
					{
						id: "cursor:cursor-models",
						label: "Cursor models",
						percent_used: clamp_percent(cursor_models_percent ?? 0),
					},
					{
						id: "cursor:other-models",
						label: "Other models",
						percent_used: clamp_percent(other_models_percent ?? 0),
					},
				]
			: [
					{
						id: "cursor:included-usage",
						label: "Included usage",
						percent_used: clamp_percent(plan_percent),
					},
				];
	const windows: Array<EngineQuotaWindow> = plan_windows.map((window) => ({
		...window,
		kind: "monthly",
		...(resets_at === undefined ? {} : { resets_at }),
		scope: "shared",
		...(window_minutes === undefined ? {} : { window_minutes }),
	}));

	const spend_limit = optional_record(response.spendLimitUsage, "spendLimitUsage");
	if (spend_limit === undefined) return windows;
	const candidates = [
		["overallLimit", "overallUsed", "overallRemaining"],
		["individualLimit", "individualUsed", "individualRemaining"],
		["pooledLimit", "pooledUsed", "pooledRemaining"],
	] as const;
	for (const [limit_key, used_key, remaining_key] of candidates) {
		const limit = optional_number(spend_limit, limit_key) ?? 0;
		if (limit <= 0) continue;
		const remaining = optional_number(spend_limit, remaining_key);
		const used =
			optional_number(spend_limit, used_key) ??
			(remaining === undefined ? 0 : Math.max(0, limit - remaining));
		windows.push({
			id: "cursor:on-demand",
			kind: "monthly",
			label: "On-demand",
			percent_used: clamp_percent((used / limit) * 100),
			...(resets_at === undefined ? {} : { resets_at }),
			scope: "shared",
			...(window_minutes === undefined ? {} : { window_minutes }),
		});
		break;
	}

	return windows;
}

/** Resolves the same Cursor CLI credential file used by the installed agent. */
export function cursor_auth_file_path(options: CursorUsageOptions = {}): string {
	if (options.auth_file !== undefined) return options.auth_file;
	const environment = options.environment ?? process.env;
	const platform = options.platform ?? process.platform;
	const home_directory = options.home_directory ?? homedir();
	if (platform === "win32") {
		const app_data = environment.APPDATA;
		return app_data === undefined || app_data.length === 0
			? join(home_directory, "AppData", "Roaming", "Cursor", "auth.json")
			: join(app_data, "Cursor", "auth.json");
	}
	if (platform === "darwin") return join(home_directory, ".cursor", "auth.json");
	const config_home = environment.XDG_CONFIG_HOME;
	return join(
		config_home === undefined || config_home.length === 0
			? join(home_directory, ".config")
			: config_home,
		"cursor",
		"auth.json",
	);
}

async function read_cursor_access_token(path: string): Promise<string | undefined> {
	let contents: string;
	try {
		contents = await readFile(path, "utf8");
	} catch (cause) {
		if ((cause as NodeJS.ErrnoException).code === "ENOENT") return undefined;
		throw cause;
	}
	if (Buffer.byteLength(contents, "utf8") > cursor_auth_file_maximum_bytes)
		throw new Error("Cursor credential file exceeds its size bound");
	const credential = as_record(JSON.parse(contents), "Cursor credential");
	return typeof credential.accessToken === "string" && credential.accessToken.length > 0
		? credential.accessToken
		: undefined;
}

/**
 * Reads Cursor's authenticated billing-period usage without starting a model
 * session. The bearer token is sent only to Cursor's fixed DashboardService
 * endpoint and is never included in an error or returned value.
 */
export function MakeCursorUsage(
	options: CursorUsageOptions = {},
): Effect.Effect<EngineAccountUsage, EngineFailure> {
	return Effect.gen(function* () {
		const token = yield* Effect.tryPromise({
			try: () =>
				options.ReadAccessToken?.() ??
				read_cursor_access_token(cursor_auth_file_path(options)),
			catch: () =>
				new EngineUnavailableError({
					engine_id: "cursor",
					message: "Cursor credentials could not be read.",
				}),
		});
		if (token === undefined) {
			return {
				authentication: {
					reason: "Sign in to Cursor from Settings.",
					state: "unauthenticated" as const,
				},
				quota_surface: "supported" as const,
				windows: [],
			} satisfies EngineAccountUsage;
		}

		const Fetch: CursorUsageFetch =
			options.Fetch ??
			((url, init) => globalThis.fetch(url, init) as Promise<CursorUsageHttpResponse>);
		const response = yield* Effect.tryPromise({
			try: () =>
				Fetch(cursor_usage_endpoint, {
					body: "{}",
					headers: {
						Authorization: `Bearer ${token}`,
						"Connect-Protocol-Version": "1",
						"Content-Type": "application/json",
						"x-cursor-client-type": "cli",
					},
					method: "POST",
					signal: AbortSignal.timeout(options.timeout_ms ?? cursor_usage_timeout_ms),
				}),
			catch: () =>
				new EngineUnavailableError({
					engine_id: "cursor",
					message: "Cursor usage could not be reached.",
				}),
		});
		if (response.status === 401 || response.status === 403) {
			return {
				authentication: {
					reason: "Cursor sign-in is no longer valid.",
					state: "unauthenticated" as const,
				},
				quota_surface: "supported" as const,
				windows: [],
			} satisfies EngineAccountUsage;
		}
		if (!response.ok)
			return yield* Effect.fail(
				new EngineUnavailableError({
					engine_id: "cursor",
					message: `Cursor usage returned HTTP ${response.status}.`,
				}),
			);

		const payload = yield* Effect.tryPromise({
			try: () => response.json(),
			catch: () =>
				new EngineProtocolError({
					engine_id: "cursor",
					message: "Cursor usage returned invalid JSON.",
				}),
		});
		const windows = yield* Effect.try({
			try: () => map_cursor_period_usage_to_quota_windows(payload),
			catch: () =>
				new EngineProtocolError({
					engine_id: "cursor",
					message: "Cursor usage returned an invalid response.",
				}),
		});
		return {
			authentication: { state: "authenticated" as const },
			quota_surface: "supported" as const,
			windows,
		} satisfies EngineAccountUsage;
	});
}

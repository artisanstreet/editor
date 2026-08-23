import { Context, Layer } from "effect";

import {
	type Engine,
	type EngineDescriptor,
	type EngineOpenInput,
	EngineUnavailableError,
} from "../engine";
import { MakeAcpEngine, type AcpEngineOptions } from "../acp/engine";
import { MakeCursorUsage, type CursorUsageOptions } from "./usage";

export const cursor_transport = "cursor-acp-stdio";

export const CursorEngineDescriptor: EngineDescriptor = {
	capabilities: {
		approval: { state: "supported", reason: "ACP permission requests are bridged to Artisan." },
		auth: { state: "supported", reason: "Uses Cursor browser or API-key authentication." },
		cancel: { state: "supported" },
		close: { state: "supported" },
		events: { state: "supported" },
		global_guidance: {
			state: "unsupported",
			reason: "Cursor discovers AGENTS.md, CLAUDE.md, and .cursor/rules natively.",
		},
		model_catalog: {
			state: "unsupported",
			reason: "Cursor models are supplied by Artisan's curated catalog.",
		},
		model_selection: {
			state: "supported",
			reason: "The selected model is passed to Cursor ACP.",
		},
		native_continuation: {
			state: "unsupported",
			reason: "ACP does not guarantee that a loaded session may change model identity.",
		},
		native_tools: {
			state: "supported",
			reason: "ACP tool calls and file locations are normalized.",
		},
		probe: { state: "supported" },
		question: {
			state: "supported",
			reason: "ACP permissions, questions, and plan approval extensions are bridged.",
		},
		raw_frames: { state: "supported" },
		resume: { state: "supported", reason: "ACP session/load reopens Cursor conversations." },
		start: { state: "supported" },
		steer: {
			state: "experimental",
			reason: "A waiting ACP session accepts a follow-up; active prompts cannot be steered in place.",
		},
		subagents: {
			state: "experimental",
			reason: "Cursor ACP exposes subagent task notifications without a child transcript stream.",
		},
	},
	display_name: "Cursor",
	id: "cursor",
	transport: cursor_transport,
};

const string_option = (input: EngineOpenInput, key: string) => {
	const value = input.provider_options?.[key];
	return typeof value === "string" && value.length > 0 ? value : undefined;
};

const cursor_reasoning_suffix = /-(?:low|medium|high|xhigh|max|ultra)(?:-fast)?$/;

/** Resolves Artisan's Cursor controls to one exact model id from `agent models`. */
export const ResolveCursorModel = (input: EngineOpenInput) => {
	if (input.model === undefined) return undefined;
	if (input.model.includes("[")) return input.model;
	const effort = string_option(input, "cursor.reasoning_effort");
	const speed = string_option(input, "cursor.speed");
	const with_effort =
		effort === undefined || cursor_reasoning_suffix.test(input.model)
			? input.model
			: `${input.model}-${effort}`;
	return speed === "fast" && !with_effort.endsWith("-fast") ? `${with_effort}-fast` : with_effort;
};

export const CursorAcpArgs = (input: EngineOpenInput) => {
	const permission = string_option(input, "cursor.permission_mode");
	const model = ResolveCursorModel(input);
	return [
		...(model === undefined ? [] : ["--model", model]),
		...(input.permission_policy?.write_access === false
			? ["--mode", "ask"]
			: permission === "force"
				? ["--force"]
				: []),
		"acp",
	];
};

export class CursorEngine extends Context.Service<CursorEngine, Engine>()("Artisan/CursorEngine") {}

export type CursorEngineOptions = Omit<AcpEngineOptions, "definition"> & {
	readonly executable?: string;
	readonly usage?: CursorUsageOptions;
};

const cursor_unavailable_model_pattern =
	/Cannot use this model:\s*([^\r\n]{1,160}?)(?:\.\s+Valid models|[\r\n]|$)/i;

/** Converts Cursor's known pre-session model rejection to Artisan's stable model code. */
export const ClassifyCursorStartupFailure = (stderr: string) => {
	const model = cursor_unavailable_model_pattern.exec(stderr)?.[1]?.trim();
	return model === undefined
		? undefined
		: new EngineUnavailableError({
				artisan_code: "AE-PROVIDER-206",
				engine_id: "cursor",
				message: `Cursor does not make model ${model} available to this account.`,
			});
};

export const make_cursor_engine_layer = (options: CursorEngineOptions = {}) =>
	Layer.effect(
		CursorEngine,
		MakeAcpEngine({
			...options,
			definition: {
				AcpArgs: CursorAcpArgs,
				Authenticated: (output) => !/not authenticated|not logged in/i.test(output),
				AuthMethod: (initialize) =>
					(initialize.authMethods ?? []).some((method) => method.id === "cursor_login")
						? "cursor_login"
						: undefined,
				auth_probe_args: ["status"],
				ClassifyStartupFailure: ({ stderr }) => ClassifyCursorStartupFailure(stderr),
				descriptor: CursorEngineDescriptor,
				executable:
					options.executable ?? (process.platform === "win32" ? "agent.cmd" : "agent"),
				image_input: "image",
				Usage: MakeCursorUsage(options.usage),
				Version: (output) =>
					/\b(\d{4}\.\d{1,2}\.\d{1,2}-[0-9A-Za-z._-]+)\b/.exec(output)?.[1],
			},
		}),
	);

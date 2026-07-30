import { createHash } from "node:crypto";

import { Effect } from "effect";
import type { CapabilityConnectPreview, CapabilityDetail } from "@artisan/protocol";

import { inspect_http_mcp_endpoint } from "./http-transport";

export const Fingerprint = (value: unknown) =>
	createHash("sha256").update(JSON.stringify(value)).digest("hex");

export const Preview = (
	input: Pick<CapabilityDetail, "auth" | "scope" | "source" | "transport">,
) => {
	const reviewed = {
		auth: input.auth,
		scope: input.scope,
		source: input.source,
		transport: input.transport,
	};
	return Effect.succeed({
		auth: input.auth,
		candidate_id: `capability_${Fingerprint(reviewed).slice(0, 24)}`,
		candidate_name: input.source.locator,
		compatibility: [{ engine_id: "codex", state: "runtime_only" }],
		discovery_status: "requires_connection",
		permissions: [
			input.transport.kind === "stdio"
				? {
						description: "Starts the reviewed local MCP command.",
						kind: "process" as const,
					}
				: {
						description: `Connects to ${new URL(input.transport.url).origin}.`,
						kind: "network" as const,
					},
			...(input.transport.kind === "stdio" && input.transport.env?.length
				? [
						{
							description: "Reads approved environment secret references.",
							kind: "environment" as const,
						},
					]
				: []),
			...(input.auth.kind === "none"
				? []
				: [
						{
							description: "Uses an approved account credential reference.",
							kind: "account" as const,
						},
					]),
		],
		preview_fingerprint: Fingerprint(reviewed),
		rollback_available: false,
		scope: input.scope,
		source: input.source,
		tools: [],
		transport: input.transport,
		...(input.transport.kind === "streamable_http"
			? {
					transport_policy: inspect_http_mcp_endpoint({
						max_response_bytes: input.transport.max_response_bytes ?? 1_048_576,
						timeout_ms: input.transport.timeout_ms ?? 30_000,
						url: input.transport.url,
					}),
				}
			: {}),
		trust:
			input.source.kind === "local" || input.source.kind === "plugin_bundle"
				? "local"
				: "unverified",
	} satisfies CapabilityConnectPreview);
};

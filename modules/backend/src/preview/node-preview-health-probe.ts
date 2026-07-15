import { lookup } from "node:dns/promises";
import type { LookupAddress } from "node:dns";
import type { LookupFunction } from "node:net";

import * as NodeHttpClient from "@effect/platform-node/NodeHttpClient";
import { Clock, Context, Effect, Layer, Option, Result, Scope } from "effect";
import * as HttpClient from "effect/unstable/http/HttpClient";

import {
	canonical_hostname,
	is_ip_literal,
	is_localhost_name,
	is_loopback_address,
	ip_address_family,
} from "./network-policy";
import {
	PreviewHealthProbe,
	PreviewHealthProbeError,
	type PreviewHealthProbeResult,
	type PreviewTargetRecord,
} from "./preview-target";

const protocol_latency_bound_ms = 600_000;
const default_timeout_ms = 5_000;

interface PreviewDnsAddress {
	readonly address: string;
	readonly family: 4 | 6;
}

/** Resolves one preview hostname for policy validation. */
export class PreviewHealthDnsResolver extends Context.Service<
	PreviewHealthDnsResolver,
	{
		readonly Lookup: (
			hostname: string,
		) => Effect.Effect<ReadonlyArray<PreviewDnsAddress>, unknown>;
	}
>()("Artisan/PreviewHealthDnsResolver") {}

/** Resolves DNS using Node's promises API for production preview probes. */
export const NodePreviewHealthDnsResolverLive = Layer.succeed(PreviewHealthDnsResolver, {
	Lookup: (hostname) =>
		Effect.tryPromise(() => lookup(hostname, { all: true, verbatim: true })).pipe(
			Effect.map((addresses) =>
				addresses.map((address) => ({
					address: address.address,
					family: address.family as 4 | 6,
				})),
			),
		),
});

export interface NodePreviewHealthProbeOptions {
	readonly timeout_ms?: number;
}

interface ValidatedTarget {
	readonly url: URL;
	readonly addresses: ReadonlyArray<PreviewDnsAddress>;
}

function failed(target_id: string) {
	return new PreviewHealthProbeError({ reason: "failed", target_id });
}

function unhealthy(
	message: string,
	latency_ms: number,
	status_code?: number,
): PreviewHealthProbeResult {
	return {
		latency_ms,
		message: Option.some(message.slice(0, 512)),
		status: "unhealthy",
		status_code: status_code === undefined ? Option.none() : Option.some(status_code),
	};
}

function healthy(latency_ms: number, status_code: number): PreviewHealthProbeResult {
	return {
		latency_ms,
		message: Option.none(),
		status: "healthy",
		status_code: Option.some(status_code),
	};
}

function valid_timeout(timeout_ms: number) {
	return Number.isSafeInteger(timeout_ms) && timeout_ms > 0 && timeout_ms <= 60_000;
}

function parse_target(target: PreviewTargetRecord): Effect.Effect<URL, PreviewHealthProbeError> {
	return Effect.try({
		try: () => new URL(target.url),
		catch: () => failed(target.target_id),
	}).pipe(
		Effect.filterOrFail(
			(url) =>
				(url.protocol === "http:" || url.protocol === "https:") &&
				url.username === "" &&
				url.password === "" &&
				(is_ip_literal(url.hostname)
					? is_loopback_address(url.hostname)
					: is_localhost_name(url.hostname)),
			() => failed(target.target_id),
		),
		Effect.mapError(() => failed(target.target_id)),
	);
}

function validate_addresses(
	url: URL,
	addresses: ReadonlyArray<PreviewDnsAddress>,
	target_id: string,
): Effect.Effect<ValidatedTarget, PreviewHealthProbeError> {
	const expected_family = is_ip_literal(url.hostname) ? ip_address_family(url.hostname) : 0;

	if (
		addresses.length === 0 ||
		addresses.some(
			(address) =>
				!is_loopback_address(address.address) ||
				(address.family !== 4 && address.family !== 6) ||
				ip_address_family(address.address) !== address.family ||
				(expected_family !== 0 && address.family !== expected_family),
		)
	) {
		return Effect.fail(failed(target_id));
	}

	return Effect.succeed({ addresses, url });
}

function pinned_lookup(addresses: ReadonlyArray<PreviewDnsAddress>): LookupFunction {
	return (_hostname: string, options: Parameters<LookupFunction>[1], callback) => {
		const requested_family =
			options.family === 4 || options.family === "IPv4"
				? 4
				: options.family === 6 || options.family === "IPv6"
					? 6
					: 0;
		const eligible_addresses =
			requested_family === 0
				? addresses
				: addresses.filter((address) => address.family === requested_family);

		if (eligible_addresses.length === 0) {
			callback(new Error("No validated preview address matches the requested family"), "", 0);

			return;
		}

		if (options.all) {
			callback(
				null,
				eligible_addresses.map(
					(address): LookupAddress => ({
						address: canonical_hostname(address.address),
						family: address.family,
					}),
				),
			);
			return;
		}

		const address = eligible_addresses[0]!;

		callback(null, canonical_hostname(address.address), address.family);
	};
}

function probe_request(validated: ValidatedTarget, timeout_ms: number) {
	const agent_layer = NodeHttpClient.layerAgentOptions({
		keepAlive: false,
		lookup: pinned_lookup(validated.addresses),
	});
	const client_layer = NodeHttpClient.layerNodeHttpNoAgent.pipe(Layer.provide(agent_layer));
	return Effect.scoped(
		Effect.provide(
			Effect.gen(function* () {
				const start_ns = yield* Clock.currentTimeNanos;
				const client = yield* HttpClient.HttpClient;
				const scoped_client = HttpClient.withScope(client);
				const response = yield* scoped_client
					.head(validated.url)
					.pipe(Effect.timeout(timeout_ms), Effect.result);
				const end_ns = yield* Clock.currentTimeNanos;
				const latency_ms = Math.max(
					0,
					Math.min(protocol_latency_bound_ms, Number((end_ns - start_ns) / 1_000_000n)),
				);

				if (Result.isFailure(response)) {
					return unhealthy("Preview request failed", latency_ms);
				}

				return response.success.status >= 100 && response.success.status <= 499
					? healthy(latency_ms, response.success.status)
					: response.success.status >= 500 && response.success.status <= 599
						? unhealthy(
								"Preview server returned an error",
								latency_ms,
								response.success.status,
							)
						: unhealthy("Preview request failed", latency_ms);
			}),
			client_layer,
		),
	);
}

/** Builds the production Node preview health probe with an injectable DNS resolver. */
export function make_node_preview_health_probe_layer(options: NodePreviewHealthProbeOptions = {}) {
	const timeout_ms = options.timeout_ms ?? default_timeout_ms;

	if (!valid_timeout(timeout_ms)) {
		return Layer.succeed(PreviewHealthProbe, {
			Probe: (target: PreviewTargetRecord) => Effect.fail(failed(target.target_id)),
		});
	}

	return Layer.effect(
		PreviewHealthProbe,
		Effect.gen(function* () {
			const dns_resolver = yield* PreviewHealthDnsResolver;
			return {
				Probe: (
					target: PreviewTargetRecord,
				): Effect.Effect<PreviewHealthProbeResult, PreviewHealthProbeError, Scope.Scope> =>
					Effect.gen(function* () {
						const url = yield* parse_target(target);
						const addresses_result = is_ip_literal(url.hostname)
							? Result.succeed([
									{
										address: canonical_hostname(url.hostname),
										family: ip_address_family(url.hostname) as 4 | 6,
									},
								])
							: yield* dns_resolver.Lookup(url.hostname).pipe(Effect.result);

						if (Result.isFailure(addresses_result)) {
							return unhealthy("Preview DNS failed", 0);
						}

						const validated = yield* validate_addresses(
							url,
							addresses_result.success,
							target.target_id,
						);

						return yield* probe_request(validated, timeout_ms);
					}).pipe(Effect.mapError(() => failed(target.target_id))),
			};
		}),
	);
}

/** Provides the production Node preview health probe and DNS resolver. */
export const NodePreviewHealthProbeLive = make_node_preview_health_probe_layer().pipe(
	Layer.provide(NodePreviewHealthDnsResolverLive),
);

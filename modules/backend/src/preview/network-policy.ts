import ipaddr from "ipaddr.js";

function normalize_hostname(hostname: string) {
	const unwrapped =
		hostname.startsWith("[") && hostname.endsWith("]") ? hostname.slice(1, -1) : hostname;

	return unwrapped.toLowerCase().replace(/\.$/, "");
}

/** Returns the hostname form accepted by DNS and TLS adapters. */
export function canonical_hostname(hostname: string) {
	return normalize_hostname(hostname);
}

/** Reports whether a hostname is an IP literal understood by the policy parser. */
export function is_ip_literal(hostname: string) {
	return ipaddr.isValid(normalize_hostname(hostname));
}

/** Returns the parsed address family, or zero when the value is not an IP. */
export function ip_address_family(address: string): 0 | 4 | 6 {
	const normalized = normalize_hostname(address);

	if (!ipaddr.isValid(normalized)) {
		return 0;
	}

	return ipaddr.parse(normalized).kind() === "ipv6" ? 6 : 4;
}

/** Allows only globally routable unicast addresses for external metadata fetches. */
export function is_public_address(address: string) {
	const normalized = normalize_hostname(address);

	if (!ipaddr.isValid(normalized)) {
		return false;
	}

	const parsed = ipaddr.parse(normalized);

	if (
		parsed.kind() === "ipv6" &&
		"isIPv4MappedAddress" in parsed &&
		parsed.isIPv4MappedAddress()
	) {
		return false;
	}

	return parsed.range() === "unicast";
}

/** Rejects localhost names before DNS resolution. */
export function is_localhost_name(hostname: string) {
	const normalized = normalize_hostname(hostname);

	return normalized === "localhost" || normalized.endsWith(".localhost");
}

/** Allows only explicitly registered localhost or loopback preview targets. */
export function is_local_preview_hostname(hostname: string) {
	const normalized = normalize_hostname(hostname);

	if (is_localhost_name(normalized)) {
		return true;
	}

	if (!ipaddr.isValid(normalized)) {
		return false;
	}

	const parsed = ipaddr.parse(normalized);

	if (
		parsed.kind() === "ipv6" &&
		"isIPv4MappedAddress" in parsed &&
		parsed.isIPv4MappedAddress()
	) {
		return parsed.toIPv4Address().range() === "loopback";
	}

	return parsed.range() === "loopback";
}

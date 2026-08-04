import type { ForgeConfig } from "./config";

const loopback_host_names = new Set(["127.0.0.1", "localhost", "[::1]"]);

/**
 * Defeats DNS rebinding for HTTP requests and WebSocket upgrades. Development
 * tooling may add exact reverse-proxy hostnames; wildcards are never accepted.
 */
export const ForgeHostAllowed = (
	host: string | undefined,
	config: Pick<ForgeConfig, "allowed_hostnames">,
): boolean => {
	if (host === undefined) return false;
	try {
		const hostname = new URL(`http://${host}`).hostname.toLowerCase();
		return loopback_host_names.has(hostname) || config.allowed_hostnames.includes(hostname);
	} catch {
		return false;
	}
};

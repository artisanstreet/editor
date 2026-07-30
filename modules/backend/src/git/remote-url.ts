import type { RepositoryHost } from "@artisan/protocol";

/**
 * Parses Git remote URLs into a host identity and a browsable page.
 *
 * Git accepts several syntaxes for the same remote, and the `scp`-like form
 * (`git@host:owner/repo.git`) is not a URL at all, so this normalizes them
 * before deciding anything. Pure and process-free: the parsing rules are the
 * part worth testing, and they should never need a repository on disk.
 *
 * @since 0.8.0
 */

/** The parts of a remote that survive normalization. */
interface RemoteParts {
	readonly hostname: string;
	/** Path with leading slash and trailing `.git` removed. */
	readonly path: string;
	/** Whether the remote addresses a network host rather than a local path. */
	readonly networked: boolean;
}

/** `git@github.com:owner/repo.git` — an `scp`-like remote, not a parsable URL. */
const SCP_LIKE = /^(?:(?<user>[^@/\\]+)@)?(?<hostname>[^:/\\]+):(?<path>[^\\].*)$/;

/** Windows drive paths (`C:\src\repo`) collide with the `scp`-like shape. */
const WINDOWS_PATH = /^[A-Za-z]:[\\/]/;

const KNOWN_HOSTS: ReadonlyArray<readonly [RegExp, RepositoryHost]> = [
	[/(^|\.)github\.com$/, "github"],
	[/(^|\.)gitlab\./, "gitlab"],
	[/^gitlab\.[^.]+$/, "gitlab"],
	[/(^|\.)bitbucket\.org$/, "bitbucket"],
	[/(^|\.)dev\.azure\.com$/, "azure"],
	[/(^|\.)visualstudio\.com$/, "azure"],
	[/(^|\.)codeberg\.org$/, "codeberg"],
	[/(^|\.)sr\.ht$/, "sourcehut"],
	[/(^|\.)gitea\./, "gitea"],
	[/^gitea\.[^.]+$/, "gitea"],
];

const StripGitSuffix = (path: string) => path.replace(/\.git$/, "");

const NormalizePath = (path: string) =>
	StripGitSuffix(path.replace(/^\/+/, "").replace(/\/+$/, ""));

/**
 * Reduces any remote syntax to comparable parts.
 *
 * @param url - A remote URL exactly as Git reports it.
 * @returns The remote's host and path, or `undefined` when it names no host.
 */
const Parse = (url: string): RemoteParts | undefined => {
	const trimmed = url.trim();
	if (trimmed === "") return undefined;

	/** A local path is a remote, but never a browsable one. */
	if (WINDOWS_PATH.test(trimmed) || trimmed.startsWith("/") || trimmed.startsWith(".")) {
		return { hostname: "", networked: false, path: NormalizePath(trimmed) };
	}

	if (trimmed.startsWith("file://")) {
		return { hostname: "", networked: false, path: NormalizePath(trimmed.slice(7)) };
	}

	const scp = SCP_LIKE.exec(trimmed);
	const scp_hostname = scp?.groups?.["hostname"];
	const scp_path = scp?.groups?.["path"];
	if (scp_hostname !== undefined && scp_path !== undefined && !trimmed.includes("://")) {
		return {
			hostname: scp_hostname.toLowerCase(),
			networked: true,
			path: NormalizePath(scp_path),
		};
	}

	try {
		const parsed = new URL(trimmed);
		return {
			hostname: parsed.hostname.toLowerCase(),
			networked: parsed.protocol !== "file:" && parsed.hostname !== "",
			path: NormalizePath(parsed.pathname),
		};
	} catch {
		return undefined;
	}
};

/**
 * Identifies the service hosting a remote.
 *
 * @param url - A remote URL exactly as Git reports it.
 * @returns The detected host, or `unknown` when the remote names none.
 */
export const RepositoryHostFor = (url: string): RepositoryHost => {
	const parts = Parse(url);
	if (parts === undefined || !parts.networked) return "unknown";

	for (const [pattern, host] of KNOWN_HOSTS) {
		if (pattern.test(parts.hostname)) return host;
	}

	return "other";
};

/**
 * Derives the page a browser should open for a remote.
 *
 * Azure DevOps is the one host whose `ssh` and web paths genuinely differ — its
 * `ssh` remotes carry a `/v3/` prefix and drop the `_git` segment — so it is
 * rewritten rather than passed through.
 *
 * @param url - A remote URL exactly as Git reports it.
 * @returns An `https` URL, or `undefined` for a remote with no web page.
 */
export const RepositoryWebUrlFor = (url: string): string | undefined => {
	const parts = Parse(url);
	if (parts === undefined || !parts.networked || parts.path === "") return undefined;

	if (RepositoryHostFor(url) === "azure") {
		const segments = parts.path.split("/").filter((segment) => segment !== "");
		/** `v3/organization/project/repository` is the ssh form. */
		const azure = segments[0] === "v3" ? segments.slice(1) : undefined;

		if (azure !== undefined && azure.length === 3) {
			return `https://${parts.hostname}/${azure[0]}/${azure[1]}/_git/${azure[2]}`;
		}
	}

	return `https://${parts.hostname}/${parts.path}`;
};

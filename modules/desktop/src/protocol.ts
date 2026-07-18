import { isAbsolute, relative, resolve } from "node:path";

/** The sole renderer origin. It supports workers without granting file:// access. */
export const DesktopRendererOrigin = "artisan://app";

/** Resolves one custom-protocol URL strictly below the packaged frontend root. */
export const resolve_frontend_request = (frontend_root: string, request_url: string) => {
	if (/%2e|%2f|%5c/i.test(request_url)) {
		return undefined;
	}

	const url = new URL(request_url);

	if (url.protocol !== "artisan:" || url.hostname !== "app") {
		return undefined;
	}

	let pathname: string;

	try {
		pathname = decodeURIComponent(url.pathname);
	} catch {
		return undefined;
	}

	if (pathname.includes("\0")) {
		return undefined;
	}

	const candidate = resolve(frontend_root, `.${pathname === "/" ? "/index.html" : pathname}`);
	const relative_path = relative(frontend_root, candidate);

	return relative_path === "" || (!relative_path.startsWith("..") && !isAbsolute(relative_path))
		? candidate
		: undefined;
};

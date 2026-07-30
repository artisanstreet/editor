import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

import { Effect, Schema } from "effect";

import { ForgeState } from "./state";

/**
 * One announced Forge process. Cards reuse the durable state shape, so a card
 * is simply that state written to the machine-global registry instead of the
 * instance's private home.
 */
export type ForgeInstanceCard = ForgeState;

/**
 * Resolves the machine-global root that every Forge announces into, regardless
 * of which `ARTISAN_HOME` it serves. Discovery across separate installs only
 * works because this location is shared: a development checkout and the
 * installed release each have private homes, but both write cards here.
 */
export const ResolveInstanceRegistryRoot = (environment: {
	readonly HOME?: string;
	readonly LOCALAPPDATA?: string;
	readonly USERPROFILE?: string;
}) => {
	const base =
		environment.LOCALAPPDATA ?? environment.USERPROFILE ?? environment.HOME ?? undefined;
	return base === undefined ? undefined : join(base, "Artisan");
};

export const InstanceCardPath = (registry_root: string, instance_id: string) =>
	join(registry_root, "instances", `${instance_id}.json`);

const alive = (pid: number) => {
	try {
		process.kill(pid, 0);
		return true;
	} catch (cause) {
		return (cause as NodeJS.ErrnoException).code === "EPERM";
	}
};

const loopback_hosts = new Set(["127.0.0.1", "localhost", "[::1]"]);

/**
 * Cards are plain files any same-user process can write, and listings end up
 * rendered as navigation targets. Only plain-HTTP loopback endpoints are
 * meaningful for a local Forge, so anything else — external hosts, other
 * schemes, javascript: URLs — is discarded at read time rather than trusted.
 */
const loopback_endpoint = (endpoint: string) => {
	try {
		const url = new URL(endpoint);
		return url.protocol === "http:" && loopback_hosts.has(url.hostname.toLowerCase());
	} catch {
		return false;
	}
};

const ReadCard = (path: string) =>
	Effect.tryPromise(() => readFile(path, "utf8")).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(Schema.fromJsonString(ForgeState))),
		Effect.map((card) => [card] as const),
		Effect.catch(() => Effect.succeed([] as ReadonlyArray<ForgeInstanceCard>)),
	);

const ListDirectory = (path: string) =>
	Effect.tryPromise(() => readdir(path, { withFileTypes: true })).pipe(
		Effect.catch(() => Effect.succeed([])),
	);

/**
 * Lists every live Forge announced under the registry root. A card whose
 * process is gone is ignored rather than deleted: removal stays with the
 * owning process so a listing can never race a startup.
 */
export const ListForgeInstances = (registry_root: string) =>
	Effect.gen(function* () {
		const card_entries = yield* ListDirectory(join(registry_root, "instances"));
		const card_reads = card_entries
			.filter((entry) => entry.isFile() && entry.name.endsWith(".json"))
			.map((entry) => ReadCard(join(registry_root, "instances", entry.name)));

		const cards = (yield* Effect.all(card_reads)).flat();
		const by_instance = new Map<string, ForgeInstanceCard>();
		for (const card of cards) {
			if (alive(card.pid) && loopback_endpoint(card.endpoint)) {
				by_instance.set(card.instance_id, card);
			}
		}
		return [...by_instance.values()];
	});

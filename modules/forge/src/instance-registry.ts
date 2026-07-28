import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

import { Effect, Schema } from "effect";

import { ForgeState } from "./state";

/**
 * One announced Forge process. Cards reuse the durable state shape, so a card
 * is simply that state written to the machine-global registry instead of the
 * profile's private home.
 */
export type ForgeInstanceCard = ForgeState;

/**
 * Resolves the machine-global root that every Forge announces into, regardless
 * of which `ARTISAN_HOME` it serves. Discovery across separate installs only
 * works because this location is shared: a development checkout and the
 * installed release each have private profile homes, but both write cards
 * here. The installed release also keeps `profiles/<name>/state.json` under
 * this same root, which lets listing see older Forges that predate cards.
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

const decode_card = Schema.decodeUnknownEffect(ForgeState);

const ReadCard = (path: string) =>
	Effect.tryPromise(() => readFile(path, "utf8")).pipe(
		Effect.flatMap((encoded) => Effect.try(() => JSON.parse(encoded) as unknown)),
		Effect.flatMap((decoded) => decode_card(decoded)),
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

		const profile_entries = yield* ListDirectory(join(registry_root, "profiles"));
		const state_reads = profile_entries
			.filter((entry) => entry.isDirectory())
			.map((entry) => ReadCard(join(registry_root, "profiles", entry.name, "state.json")));

		const cards = (yield* Effect.all([...card_reads, ...state_reads])).flat();
		const by_instance = new Map<string, ForgeInstanceCard>();
		for (const card of cards) {
			if (alive(card.pid)) by_instance.set(card.instance_id, card);
		}
		return [...by_instance.values()];
	});

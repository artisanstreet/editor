<script lang="ts" effect>
	/**
	 * Attaching a folder, which is the only way a project comes into existence.
	 *
	 * There is no surface here: opening hands straight to the system folder
	 * picker, which already is the whole interaction — a card narrating "the
	 * picker is opening" was a second dialog explaining the first. Forge owns
	 * the catalog and the filesystem both, so the identity that comes back is
	 * one Forge minted, and nothing in this component ever spells a path.
	 */
	import { Effect } from "effect";
	import type { Project } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";

	let {
		onattached,
		open = $bindable(false),
	}: {
		/** The attached project, handed back so the surface can point itself at it. */
		onattached: (project: Project) => Effect.Effect<void>;
		open?: boolean;
	} = $props();

	const client = yield* ArtisanClient;

	/**
	 * Every opening owns one generation. The native dialog can settle after
	 * another gesture, after the picker has been closed, or after the component
	 * has gone away; only the newest opening may publish.
	 */
	let request_generation = 0;

	const BeginRequest = () => {
		request_generation += 1;
		return request_generation;
	};

	const IsCurrentRequest = (generation: number) =>
		open && generation === request_generation;

	/**
	 * A refused or failed attachment closes like a cancel: with no surface of
	 * its own there is nothing to fall back to, and the gesture that opened the
	 * picker is still there to try again.
	 */
	const AttachDirectory = (directory_id: string, generation: number) =>
		Effect.gen(function* () {
			const project = yield* client.SelectProjectDirectory({ directory_id }).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
						return undefined;
					}),
				),
			);
			if (!IsCurrentRequest(generation)) return;
			open = false;
			if (project !== undefined) yield* onattached(project);
		});

	/**
	 * Opening asks the system for a folder afresh, and closing invalidates
	 * whatever was still in flight: the generation bump on the close rerun is
	 * what keeps a picker answered after dismissal from attaching anything.
	 */
	const Reset = (is_open: boolean) =>
		Effect.gen(function* () {
			const generation = BeginRequest();
			if (!is_open) return;
			const picked = yield* client.PickProjectDirectory.pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
						return undefined;
					}),
				),
			);
			if (!IsCurrentRequest(generation)) return;
			if (picked === undefined || picked.status === "cancelled") {
				open = false;
				return;
			}
			yield* AttachDirectory(picked.directory.directory_id, generation);
		});
	/**
	 * The system picker responds long after this statement reruns. SER binds
	 * this fiber to the component scope, so teardown interrupts picker and
	 * attach I/O.
	 */
	yield* Reset(open).pipe(Effect.forkScoped);
</script>

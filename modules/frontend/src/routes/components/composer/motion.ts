import { tick } from "svelte";
import { Effect } from "effect";

import { MarkComposerAttachmentBumps } from "./dom";

/**
 * The composer's attachment choreography: the rustle that answers images
 * landing and the bump that answers a re-paste. Both live on component state
 * the composer owns, reached through the setters, so only the motion timing
 * is housed here.
 */
export const MakeComposerMotion = (options: {
	readonly Editor: () => HTMLDivElement | null;
	readonly SetBumped: (ids: ReadonlySet<string>) => void;
	readonly SetRustling: (rustling: boolean) => void;
}) => {
	let bump_generation = 0;

	/**
	 * Replays the favourite rustle on the composer whenever images land. The
	 * flag drops for a frame first so a second paste restarts the animation
	 * instead of joining one already running.
	 */
	const StartRustle = Effect.gen(function* () {
		options.SetRustling(false);
		yield* Effect.promise(() => tick());
		options.SetRustling(true);
	});
	const EndRustle = Effect.gen(function* () {
		options.SetRustling(false);
	});

	/**
	 * Shakes the attachments a re-paste was answered by. The flag drops for a
	 * frame first so spamming the same image shakes it every time rather than
	 * once, and the generation makes the newest bump own the reset.
	 */
	const BumpAttachments = (ids: ReadonlyArray<string>) =>
		Effect.gen(function* () {
			const generation = (bump_generation += 1);
			options.SetBumped(new Set());
			yield* MarkComposerAttachmentBumps(options.Editor(), []);
			yield* Effect.promise(() => tick());
			if (generation !== bump_generation) return;
			options.SetBumped(new Set(ids));
			yield* MarkComposerAttachmentBumps(options.Editor(), ids);
			yield* Effect.sleep("420 millis");
			if (generation !== bump_generation) return;
			options.SetBumped(new Set());
			yield* MarkComposerAttachmentBumps(options.Editor(), []);
		});

	return { BumpAttachments, EndRustle, StartRustle };
};

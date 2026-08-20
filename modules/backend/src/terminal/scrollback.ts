import { Deferred, Effect, Ref, Stream } from "effect";

/** One retained span of terminal output, ordered by a monotonic per-terminal sequence. */
interface ScrollbackChunk {
	readonly bytes: Uint8Array;
	readonly seq: number;
}

interface ScrollbackState {
	readonly chunks: ReadonlyArray<ScrollbackChunk>;
	readonly done: boolean;
	readonly next_seq: number;
	readonly total_bytes: number;
}

/**
 * Retains the recent output of one terminal generation so readers can replay
 * it at any time — while the process runs, from several viewers at once, and
 * after the process has exited. The driver's live stream is consume-once; this
 * buffer is the durable side of it.
 */
export interface TerminalScrollback {
	readonly generation: number;
	readonly latch: Ref.Ref<Deferred.Deferred<void>>;
	readonly state: Ref.Ref<ScrollbackState>;
	readonly thread_id: string;
	readonly workspace_id: string;
}

function concat_chunks(chunks: ReadonlyArray<ScrollbackChunk>) {
	if (chunks.length === 1) {
		return chunks[0]!.bytes;
	}

	const merged = new Uint8Array(chunks.reduce((total, chunk) => total + chunk.bytes.length, 0));
	let offset = 0;

	for (const chunk of chunks) {
		merged.set(chunk.bytes, offset);
		offset += chunk.bytes.length;
	}

	return merged;
}

export const MakeTerminalScrollback = (input: {
	readonly generation: number;
	readonly thread_id: string;
	readonly workspace_id: string;
}): Effect.Effect<TerminalScrollback> =>
	Effect.gen(function* () {
		const latch = yield* Ref.make(yield* Deferred.make<void>());
		const state = yield* Ref.make<ScrollbackState>({
			chunks: [],
			done: false,
			next_seq: 0,
			total_bytes: 0,
		});

		return { ...input, latch, state };
	});

/**
 * Readers park on the latch current at their read time; appends resolve that
 * latch after the state update, so a reader can never observe "nothing pending"
 * and then sleep through the append that just landed.
 */
const WakeReaders = (scrollback: TerminalScrollback) =>
	Effect.gen(function* () {
		const fresh = yield* Deferred.make<void>();
		const parked = yield* Ref.getAndSet(scrollback.latch, fresh);

		yield* Deferred.succeed(parked, undefined);
	});

/**
 * Appends one output chunk, evicting the oldest chunks beyond the byte limit.
 * The newest chunk always survives, so a single oversized chunk is retained
 * rather than leaving the buffer empty.
 */
export const AppendScrollback = (
	scrollback: TerminalScrollback,
	bytes: Uint8Array,
	limit_bytes: number,
) =>
	Effect.gen(function* () {
		if (bytes.length === 0) {
			return;
		}

		yield* Ref.update(scrollback.state, (current) => {
			const appended = [...current.chunks, { bytes, seq: current.next_seq }];
			let total_bytes = current.total_bytes + bytes.length;
			let evicted = 0;

			while (total_bytes > limit_bytes && evicted < appended.length - 1) {
				total_bytes -= appended[evicted]!.bytes.length;
				evicted += 1;
			}

			return {
				chunks: evicted === 0 ? appended : appended.slice(evicted),
				done: current.done,
				next_seq: current.next_seq + 1,
				total_bytes,
			};
		});
		yield* WakeReaders(scrollback);
	});

/** Marks the terminal's output complete so following readers finish instead of parking. */
export const FinishScrollback = (scrollback: TerminalScrollback) =>
	Effect.gen(function* () {
		yield* Ref.update(scrollback.state, (current) => ({ ...current, done: true }));
		yield* WakeReaders(scrollback);
	});

/**
 * Replays the retained output and then follows live appends until the terminal
 * finishes. Reading never consumes: any number of readers can follow at once,
 * and a reader whose cursor fell behind eviction resumes at the oldest retained
 * chunk.
 */
export const FollowScrollback = (scrollback: TerminalScrollback): Stream.Stream<Uint8Array> =>
	Stream.unfold(0, (cursor) =>
		Effect.gen(function* () {
			while (true) {
				const parked = yield* Ref.get(scrollback.latch);
				const current = yield* Ref.get(scrollback.state);
				const pending = current.chunks.filter((chunk) => chunk.seq >= cursor);

				if (pending.length > 0) {
					return [concat_chunks(pending), pending.at(-1)!.seq + 1] as const;
				}

				if (current.done) {
					return undefined;
				}

				yield* Deferred.await(parked);
			}
		}),
	);

/** Returns only the output retained so far, ending without waiting for more. */
export const SnapshotScrollback = (
	scrollback: TerminalScrollback,
): Effect.Effect<Stream.Stream<Uint8Array>> =>
	Ref.get(scrollback.state).pipe(
		Effect.map((current) =>
			current.chunks.length === 0
				? Stream.empty
				: Stream.succeed(concat_chunks(current.chunks)),
		),
	);

/**
 * Fixtures for the starting-surface drafts. These are drafts: nothing here
 * talks to Forge, because a design study that needs a running backend cannot
 * be looked at on a whim, and the shapes below are the only part of the real
 * catalog a layout decision depends on — a name, a root, what the repository
 * is doing, and how recently anyone worked there.
 *
 * Deliberately more projects than anyone has, so the layouts are judged at the
 * width where they actually break rather than at three comfortable rows.
 */

export type DraftProject = {
	/** 0..1 per day, oldest first — the shape a sparkline or a grid renders. */
	readonly activity: ReadonlyArray<number>;
	readonly ahead: number;
	readonly branch: string;
	/** Uncommitted files. Zero means clean, and clean is worth showing. */
	readonly dirty: number;
	/** Positions the project on the surface's colour wheel; never a literal fill. */
	readonly hue: number;
	readonly last_used: string;
	readonly name: string;
	readonly path: string;
	readonly project_id: string;
	readonly threads: number;
};

export type DraftEngine = "claude" | "codex" | "gemini" | "cursor";

export type DraftThreadState = "asking" | "idle" | "running" | "unread";

export type DraftThread = {
	readonly engine: DraftEngine;
	readonly project_id: string;
	readonly state: DraftThreadState;
	readonly thread_id: string;
	readonly title: string;
	readonly when: string;
};

/**
 * Deterministic, so a hot reload does not reshuffle the drafts halfway through
 * a review and turn a layout comparison into a data comparison.
 */
const Noise = (seed: number, index: number): number => {
	const value = Math.sin((seed + 1) * 12.9898 + index * 78.233) * 43758.5453;
	return value - Math.floor(value);
};

/** Bursty rather than uniform: real work arrives in stretches, with dead days. */
const ActivityFor = (seed: number, days: number): ReadonlyArray<number> =>
	Array.from({ length: days }, (_, index) => {
		const base = Noise(seed, index);
		const burst = Noise(seed + 7, Math.floor(index / 4));
		return base * burst < 0.12 ? 0 : Math.min(1, base * burst * 2.4);
	});

/** Every study needs a current project; the deterministic fixture always supplies one. */
export const DraftProjects: readonly [DraftProject, ...Array<DraftProject>] = [
	{
		activity: ActivityFor(1, 28),
		ahead: 3,
		branch: "master",
		dirty: 41,
		hue: 24,
		last_used: "now",
		name: "artisan-editor",
		path: "~/Desktop/artisan-editor",
		project_id: "p-artisan",
		threads: 128,
	},
	{
		activity: ActivityFor(2, 28),
		ahead: 0,
		branch: "main",
		dirty: 0,
		hue: 268,
		last_used: "2 hr ago",
		name: "svelte-effect-runtime",
		path: "~/Desktop/svelte-effect-runtime",
		project_id: "p-ser",
		threads: 46,
	},
	{
		activity: ActivityFor(3, 28),
		ahead: 1,
		branch: "release/0.9",
		dirty: 4,
		hue: 152,
		last_used: "yesterday",
		name: "barekey",
		path: "~/dev/barekey",
		project_id: "p-barekey",
		threads: 31,
	},
	{
		activity: ActivityFor(4, 28),
		ahead: 0,
		branch: "main",
		dirty: 2,
		hue: 200,
		last_used: "3 days ago",
		name: "artisan.st",
		path: "~/dev/artisan-street",
		project_id: "p-street",
		threads: 12,
	},
	{
		activity: ActivityFor(5, 28),
		ahead: 12,
		branch: "feat/queue",
		dirty: 9,
		hue: 45,
		last_used: "last week",
		name: "compiler-queue",
		path: "~/dev/compiler-queue",
		project_id: "p-queue",
		threads: 8,
	},
	{
		activity: ActivityFor(6, 28),
		ahead: 0,
		branch: "main",
		dirty: 0,
		hue: 330,
		last_used: "2 weeks ago",
		name: "ae-installer",
		path: "~/dev/ae-installer",
		project_id: "p-installer",
		threads: 5,
	},
	{
		activity: ActivityFor(7, 28),
		ahead: 0,
		branch: "trunk",
		dirty: 0,
		hue: 96,
		last_used: "a month ago",
		name: "sigurd-type",
		path: "~/dev/type/sigurd",
		project_id: "p-sigurd",
		threads: 2,
	},
];

export const DraftThreads: ReadonlyArray<DraftThread> = [
	{
		engine: "claude",
		project_id: "p-artisan",
		state: "running",
		thread_id: "t-1",
		title: "Clicking on threads is really slow — it stops the world",
		when: "12 hr ago",
	},
	{
		engine: "claude",
		project_id: "p-artisan",
		state: "asking",
		thread_id: "t-2",
		title: "The waiting for claude/codex first message has no feedback",
		when: "12 hr ago",
	},
	{
		engine: "codex",
		project_id: "p-artisan",
		state: "unread",
		thread_id: "t-3",
		title: "In threads, why are collapsibles forced open and cannot close?",
		when: "12 hr ago",
	},
	{
		engine: "cursor",
		project_id: "p-ser",
		state: "idle",
		thread_id: "t-4",
		title: "Defer event-handler expressions past the dispatch",
		when: "yesterday",
	},
	{
		engine: "claude",
		project_id: "p-ser",
		state: "idle",
		thread_id: "t-5",
		title: "Yield sites should not collect closure identifiers",
		when: "yesterday",
	},
	{
		engine: "gemini",
		project_id: "p-barekey",
		state: "unread",
		thread_id: "t-6",
		title: "Inset-shadow utility drifts on artwork surfaces",
		when: "2 days ago",
	},
	{
		engine: "codex",
		project_id: "p-street",
		state: "idle",
		thread_id: "t-7",
		title: "Point the apex at Cloudflare and keep Netim as registrar",
		when: "3 days ago",
	},
	{
		engine: "claude",
		project_id: "p-queue",
		state: "idle",
		thread_id: "t-8",
		title: "REAPI cache-first before we rent a single builder",
		when: "last week",
	},
];

/**
 * Engine identity, reduced to what a landing surface can show: a one-glyph
 * mark and a tone. The real app derives both from the runtime catalog.
 */
export const EnginePresentation: Record<
	DraftEngine,
	{ readonly glyph: string; readonly label: string; readonly tone: string }
> = {
	claude: { glyph: "✳", label: "Claude Opus 5", tone: "oklch(0.72 0.14 45)" },
	codex: { glyph: "◇", label: "GPT-5.5 Codex", tone: "oklch(0.78 0.02 260)" },
	cursor: { glyph: "▲", label: "Grok 4.5", tone: "oklch(0.7 0.13 300)" },
	gemini: { glyph: "◈", label: "Gemini 3 Pro", tone: "oklch(0.72 0.13 235)" },
};

/** The row dot's colour, by what the thread is waiting on. */
export const ThreadStateTone: Record<DraftThreadState, string> = {
	asking: "var(--question-to)",
	idle: "transparent",
	running: "var(--banner-success)",
	unread: "var(--unread)",
};

export const ThreadsFor = (project_id: string): ReadonlyArray<DraftThread> =>
	DraftThreads.filter((thread) => thread.project_id === project_id);

/** Two letters is the most a small square can hold and still be read. */
export const MonogramFor = (name: string): string => {
	const parts = name.split(/[-_. ]/).filter((part) => part.length > 0);
	return parts.length === 1
		? name.slice(0, 2).toUpperCase()
		: `${parts[0]?.[0] ?? ""}${parts[1]?.[0] ?? ""}`.toUpperCase();
};

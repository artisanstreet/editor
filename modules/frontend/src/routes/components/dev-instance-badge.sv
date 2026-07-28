<script lang="ts">
	import { onMount } from "svelte";

	import { DevInstanceProfile, DevMarkedTitle } from "$lib/root/dev-instance";

	/**
	 * A non-`default` `/health` profile means this renderer is talking to a
	 * development Forge pointed at a separate data root — whether the page is
	 * the built bundle served by that Forge or the HMR dev server proxying to
	 * it. Showing that removes the standing ambiguity between an installed app
	 * and a repository build, which otherwise look identical and differ only in
	 * which database they read.
	 */
	let profile = $state<string | undefined>(undefined);

	onMount(() => {
		let cancelled = false;

		const Probe = async () => {
			/**
			 * A page can load while its Forge is mid-restart, so one failed probe
			 * must not hide the badge for the whole session. A short retry
			 * ladder settles the answer; the desktop shell serves the renderer
			 * from `artisan://app` where this never resolves, and a permanently
			 * missing badge is the correct outcome there.
			 */
			for (let attempt = 0; attempt < 5 && !cancelled; attempt += 1) {
				try {
					const response = await fetch("/health", { cache: "no-store" });
					if (response.ok) {
						const body: unknown = await response.json();
						const named = DevInstanceProfile(body);
						if (!cancelled && named !== undefined) profile = named;
						return;
					}
				} catch {
					/** Unreachable this attempt; try again shortly. */
				}
				await new Promise((settle) => setTimeout(settle, 2_000));
			}
		};

		void Probe();
		return () => {
			cancelled = true;
		};
	});

	/**
	 * Routes own their titles through `svelte:head`, so navigation rewrites the
	 * whole title after this component has marked it. Observing the head keeps
	 * the `[Dev]` marker on every route-owned title for as long as the page is
	 * known to face a development Forge; the idempotent prefix cannot loop with
	 * its own mutations.
	 */
	$effect(() => {
		if (profile === undefined) return;

		const Mark = () => {
			const marked = DevMarkedTitle(document.title);
			if (document.title !== marked) document.title = marked;
		};

		Mark();
		const observer = new MutationObserver(Mark);
		observer.observe(document.head, { characterData: true, childList: true, subtree: true });
		return () => observer.disconnect();
	});
</script>

{#if profile !== undefined}
	<div class="dev-instance-badge" role="note" aria-label={`Development Forge profile ${profile}`}>
		<span class="dev-instance-badge-dot" aria-hidden="true"></span>
		<span class="dev-instance-badge-label">dev</span>
		<span class="dev-instance-badge-profile">{profile}</span>
	</div>
{/if}

<style>
	.dev-instance-badge {
		position: fixed;
		right: 0.75rem;
		bottom: 0.75rem;
		z-index: 2147483000;
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
		padding: 0.25rem 0.55rem;
		border-radius: 999px;
		font-size: 0.6875rem;
		line-height: 1;
		letter-spacing: 0.01em;
		/** Pointer-transparent so it can never intercept a click in the shell. */
		pointer-events: none;
		user-select: none;
		color: rgb(255 255 255 / 0.72);
		background: rgb(180 83 9 / 0.28);
		border: 0.5px solid rgb(251 146 60 / 0.45);
		backdrop-filter: blur(6px);
	}
	.dev-instance-badge-dot {
		width: 0.375rem;
		height: 0.375rem;
		border-radius: 999px;
		background: rgb(251 146 60);
	}
	.dev-instance-badge-label {
		font-weight: 600;
		text-transform: uppercase;
	}
	.dev-instance-badge-profile {
		color: rgb(255 255 255 / 0.5);
	}
</style>

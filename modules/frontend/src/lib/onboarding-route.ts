export const ShouldRedirectToOnboarding = (input: {
	readonly completed: boolean | undefined;
	readonly defaults_available: boolean;
	readonly pathname: string;
}): boolean =>
	input.defaults_available &&
	input.completed !== true &&
	input.pathname !== "/onboarding" &&
	input.pathname !== "/debug" &&
	!input.pathname.startsWith("/debug/");

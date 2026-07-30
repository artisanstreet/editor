import { describe, expect, it } from "vitest";

import { RepositoryHostFor, RepositoryWebUrlFor } from "../../modules/backend/src/git/remote-url";

describe("git remote host detection", () => {
	it.each([
		["git@github.com:sandersonstabo/artisan.git", "github"],
		["https://github.com/sandersonstabo/artisan.git", "github"],
		["ssh://git@github.com/sandersonstabo/artisan.git", "github"],
		["git://github.com/sandersonstabo/artisan.git", "github"],
		["https://gitlab.com/group/sub/project.git", "gitlab"],
		["git@gitlab.internal.example.com:team/service.git", "gitlab"],
		["git@bitbucket.org:team/repo.git", "bitbucket"],
		["https://dev.azure.com/org/project/_git/repo", "azure"],
		["https://org.visualstudio.com/project/_git/repo", "azure"],
		["https://codeberg.org/owner/repo.git", "codeberg"],
		["https://git.sr.ht/~owner/repo", "sourcehut"],
		["https://gitea.example.com/owner/repo.git", "gitea"],
		["https://git.example.com/owner/repo.git", "other"],
	] as const)("reads %s as %s", (url, host) => {
		expect(RepositoryHostFor(url)).toBe(host);
	});

	it.each([
		["/home/sander/code/artisan"],
		["C:\\Users\\sander\\Desktop\\artisan-editor"],
		["file:///home/sander/code/artisan"],
		["../sibling-checkout"],
		[""],
	] as const)("treats %s as having no host", (url) => {
		expect(RepositoryHostFor(url)).toBe("unknown");
	});

	it("does not mistake a Windows path for an scp-like remote", () => {
		/** `C:\src\repo` matches the `host:path` shape but names no host. */
		expect(RepositoryHostFor("C:\\src\\repo")).toBe("unknown");
		expect(RepositoryWebUrlFor("C:\\src\\repo")).toBeUndefined();
	});
});

describe("git remote web url", () => {
	it.each([
		["git@github.com:sandersonstabo/artisan.git", "https://github.com/sandersonstabo/artisan"],
		[
			"https://github.com/sandersonstabo/artisan.git",
			"https://github.com/sandersonstabo/artisan",
		],
		[
			"ssh://git@github.com/sandersonstabo/artisan.git",
			"https://github.com/sandersonstabo/artisan",
		],
		["https://gitlab.com/group/sub/project.git", "https://gitlab.com/group/sub/project"],
		["git@bitbucket.org:team/repo.git", "https://bitbucket.org/team/repo"],
		["https://git.sr.ht/~owner/repo", "https://git.sr.ht/~owner/repo"],
	] as const)("translates %s", (url, web_url) => {
		expect(RepositoryWebUrlFor(url)).toBe(web_url);
	});

	it("rewrites an Azure DevOps ssh remote into its web form", () => {
		/** Azure ssh paths carry a `/v3/` prefix and omit the `_git` segment. */
		expect(RepositoryWebUrlFor("git@ssh.dev.azure.com:v3/org/project/repo")).toBe(
			"https://ssh.dev.azure.com/org/project/_git/repo",
		);
	});

	it("passes an Azure DevOps https remote through unchanged", () => {
		expect(RepositoryWebUrlFor("https://dev.azure.com/org/project/_git/repo")).toBe(
			"https://dev.azure.com/org/project/_git/repo",
		);
	});

	it.each([
		["/home/sander/code/artisan"],
		["file:///home/sander/code/artisan"],
		["C:\\Users\\sander\\Desktop\\artisan-editor"],
		[""],
	] as const)("offers no page for %s", (url) => {
		expect(RepositoryWebUrlFor(url)).toBeUndefined();
	});

	it("offers no page for a host without a repository path", () => {
		expect(RepositoryWebUrlFor("https://github.com/")).toBeUndefined();
	});

	it("strips only a trailing .git, never an interior one", () => {
		expect(RepositoryWebUrlFor("git@github.com:owner/my.git.repo.git")).toBe(
			"https://github.com/owner/my.git.repo",
		);
	});
});

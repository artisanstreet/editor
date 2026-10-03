# Recent-thread project icons

The icon beside a thread's project subtitle is 14 pixels wide. Disk-only
projects and Git repositories without a network remote show a folder.

For a remote repository, the first available icon wins:

1. A project file, checked in this order: `.artisan/icon.png`,
   `.artisan/icon.webp`, `icon.png`, `favicon.png`, `public/favicon.png`.
2. Repository artwork supplied by the Git host.
3. Organization, namespace, or user artwork supplied by the Git host.
4. The hosting service's bundled logo, or the Git logo for unknown hosts.

GitHub resolves the public organization/user avatar. GitLab, Bitbucket,
Codeberg, and Gitea resolve repository artwork before owner artwork. GitHub
has no dedicated repository avatar endpoint, so a local project file provides
its project-specific artwork. Private or inaccessible metadata requires no
login and falls back to the logo. No credentials are sent by this resolver.

Forge discovers and downloads icons in the background. Subtitles and icons
come from cached repository observations; listing threads and painting the
sidebar perform no Git, file, or network I/O. Successful artwork is refreshed
after an hour, absent artwork after five minutes. Changes are pushed through
the existing recent-thread subscription. A changed remote invalidates the
previous icon. Detached projects leave the cache.

Downloaded input is capped at 512 KiB and decoding runs off the Forge event
loop with image allocation and dimension limits. Artwork is normalized to a
PNG within 48 by 48 pixels and 16 KiB before crossing the protocol. Editor
reuses the decoded image identity between repaints. The added protocol fields
are optional, so an older Forge supplies folder fallback and an older Editor
ignores the artwork.

Metadata field references: [GitHub account avatars](https://docs.github.com/en/rest/users/users#get-a-user),
[GitLab project and namespace avatars](https://docs.gitlab.com/api/projects/),
and [Gitea repository metadata](https://docs.gitea.com/api/operations/repo-get/).

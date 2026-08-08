load("@aspect_rules_js//js:defs.bzl", "js_library")
load("@npm//:defs.bzl", "npm_link_all_packages")

_WORKSPACE_EXCLUDES = [
    ".git/**",
    ".pnpm-store/**",
    ".dist/**",
    ".svelte-kit/**",
    "node_modules/**",
    "target/**",
    "bazel-*/**",
]

_ALL_WORKSPACE_PACKAGES = [
    "backend",
    "catalog",
    "cli",
    "data",
    "desktop",
    "dev-tui",
    "distribution",
    "engines",
    "forge",
    "frontend",
    "installer",
    "protocol",
    "transport",
]

_BAZEL_SUPPORT_FILES = [
    "//tools/bazel:bazel-lib-windows-native-patch.patch",
    "//tools/bazel:defs.bzl",
    "//tools/bazel:run-pnpm-script.ts",
]

# pnpm lifecycle scripts need the Windows command interpreter even when the
# action otherwise rejects the ambient shell environment. This allowlisted OS
# root is an explicit action input and therefore participates in cache keys.
_WINDOWS_SHELL_ENV = ["SYSTEMROOT"]

def _workspace_targets(packages, target):
    return ["//modules/%s:%s" % (package, target) for package in packages]

def _pnpm_script_impl(ctx):
    node = ctx.toolchains["@rules_nodejs//nodejs:toolchain_type"].nodeinfo.node
    if not node:
        fail("Artisan pnpm actions require the hermetic Node toolchain")
    pnpm_files = ctx.attr._pnpm[DefaultInfo].files.to_list()
    if len(pnpm_files) != 1:
        fail("Expected pnpm to provide one package tree")
    pnpm_package = pnpm_files[0]
    node_modules_files = ctx.attr.node_modules[DefaultInfo].files.to_list()
    workspace = ctx.actions.declare_directory("%s.workspace" % ctx.label.name)
    inputs_manifest = ctx.actions.declare_file("%s.inputs.json" % ctx.label.name)
    sources_by_path = {}
    for source in ctx.files.srcs + node_modules_files:
        if source.short_path.startswith("../"):
            fail("Workspace source escaped the main repository: %s" % source.short_path)
        sources_by_path[source.short_path] = source
    source_entries = [
        {
            "relative": relative,
            "source": sources_by_path[relative].path,
        }
        for relative in sorted(sources_by_path.keys())
    ]
    ctx.actions.write(inputs_manifest, json.encode(source_entries))
    action_env = dict(ctx.attr.env)
    if not ctx.attr.use_default_shell_env:
        for name in _WINDOWS_SHELL_ENV:
            if name in ctx.configuration.default_shell_env:
                action_env[name] = ctx.configuration.default_shell_env[name]
        if ctx.attr.use_host_path and "PATH" in ctx.configuration.default_shell_env:
            action_env["PATH"] = ctx.configuration.default_shell_env["PATH"]
    ctx.actions.run(
        arguments = [
            ctx.file._runner.path,
            pnpm_package.path + "/bin/pnpm.cjs",
            ctx.attr.script,
            workspace.path,
            inputs_manifest.path,
            json.encode(ctx.attr.publish),
            json.encode(ctx.attr.workspace_packages),
            json.encode(ctx.attr.use_default_shell_env),
            json.encode(ctx.attr.use_host_path),
        ],
        env = action_env,
        executable = node,
        execution_requirements = ctx.attr.execution_requirements,
        inputs = depset(ctx.files.srcs + node_modules_files + [
            inputs_manifest,
            pnpm_package,
            ctx.file._runner,
        ]),
        mnemonic = "ArtisanPnpm",
        outputs = [workspace],
        progress_message = "Running pnpm %{label}",
        tools = [node],
        # Host-tool integration tests opt into Bazel's explicitly allowlisted
        # action environment so PATH is present. The runner still receives
        # use_default_shell_env=False and replaces home/temp with action-local
        # paths; this capability does not silently make test state host-global.
        use_default_shell_env = ctx.attr.use_default_shell_env or ctx.attr.use_host_path,
    )
    return [DefaultInfo(files = depset([workspace]))]

_pnpm_script = rule(
    implementation = _pnpm_script_impl,
    attrs = {
        "_pnpm": attr.label(
            default = "@pnpm//:pkg",
        ),
        "_runner": attr.label(
            allow_single_file = True,
            default = "//tools/bazel:run-pnpm-script.ts",
        ),
        "env": attr.string_dict(),
        "execution_requirements": attr.string_dict(),
        "node_modules": attr.label(mandatory = True),
        "publish": attr.string_list(),
        "script": attr.string(mandatory = True),
        "srcs": attr.label_list(allow_files = True),
        "use_default_shell_env": attr.bool(),
        "use_host_path": attr.bool(),
        "workspace_packages": attr.string_list(),
    },
    toolchains = ["@rules_nodejs//nodejs:toolchain_type"],
)

def pnpm_script(
        name,
        script,
        publish = [],
        tags = [],
        use_default_shell_env = False,
        use_host_path = False,
        workspace_packages = _ALL_WORKSPACE_PACKAGES):
    """Runs one existing root pnpm script with hermetic Node and translated packages."""
    _pnpm_script(
        name = name,
        node_modules = ":node_modules",
        publish = publish,
        script = script,
        srcs = _BAZEL_SUPPORT_FILES + _workspace_targets(workspace_packages, "pkg") + _workspace_targets(workspace_packages, "node_modules") + native.glob(["**"], exclude = _WORKSPACE_EXCLUDES),
        env = {
            "CI": "1",
            "NO_UPDATE_NOTIFIER": "1",
            "npm_config_verify_deps_before_run": "false",
        },
        execution_requirements = {"no-remote": "1"},
        tags = tags,
        use_default_shell_env = use_default_shell_env,
        use_host_path = use_host_path,
        workspace_packages = workspace_packages,
    )

def workspace_package():
    """Exposes one pnpm workspace as the `pkg` target rules_js links locally."""
    npm_link_all_packages(name = "node_modules")
    js_library(
        name = "pkg",
        srcs = native.glob(["**"], exclude = _WORKSPACE_EXCLUDES),
        visibility = ["//visibility:public"],
    )

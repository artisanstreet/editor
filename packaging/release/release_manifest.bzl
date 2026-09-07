"""Generate-only release manifest action for a declared archive."""


def _release_manifest_impl(ctx):
    output = ctx.actions.declare_file(ctx.attr.output_name)
    args = ctx.actions.args()
    args.add("generate")
    args.add("--archive")
    args.add(ctx.file.archive.path)
    args.add("--output")
    args.add(output.path)
    args.add("--format-version")
    args.add(str(ctx.attr.format_version))
    args.add("--product-version")
    args.add(ctx.attr.product_version)
    args.add("--editor-forge-compatibility-version")
    args.add(ctx.attr.editor_forge_compatibility_version)
    args.add("--channel")
    args.add(ctx.attr.channel)
    args.add("--signing-key-id")
    args.add(ctx.attr.signing_key_id)
    args.add("--algorithm")
    args.add(ctx.attr.algorithm)
    args.add("--minimum-installer-version")
    args.add(ctx.attr.minimum_installer_version)
    args.add("--minimum-cli-version")
    args.add(ctx.attr.minimum_cli_version)
    args.add("--artifact-id")
    args.add(ctx.attr.artifact_id)
    args.add("--platform")
    args.add(ctx.attr.platform)
    args.add("--architecture")
    args.add(ctx.attr.architecture)
    if ctx.attr.libc:
        args.add("--libc")
        args.add(ctx.attr.libc)
    args.add("--archive-format")
    args.add(ctx.attr.archive_format)
    args.add("--file-name")
    args.add(ctx.attr.file_name)

    ctx.actions.run(
        executable = ctx.executable._tool,
        inputs = depset([ctx.file.archive]),
        outputs = [output],
        tools = [ctx.executable._tool],
        arguments = [args],
        mnemonic = "ReleaseManifest",
        progress_message = "Generating release manifest %{label}",
    )
    return [DefaultInfo(files = depset([output]))]


release_manifest = rule(
    implementation = _release_manifest_impl,
    attrs = {
        "archive": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "format_version": attr.int(mandatory = True),
        "product_version": attr.string(mandatory = True),
        "editor_forge_compatibility_version": attr.string(mandatory = True),
        "channel": attr.string(mandatory = True),
        "signing_key_id": attr.string(mandatory = True),
        "algorithm": attr.string(mandatory = True),
        "minimum_installer_version": attr.string(mandatory = True),
        "minimum_cli_version": attr.string(mandatory = True),
        "artifact_id": attr.string(mandatory = True),
        "platform": attr.string(mandatory = True),
        "architecture": attr.string(mandatory = True),
        "libc": attr.string(default = ""),
        "archive_format": attr.string(mandatory = True),
        "file_name": attr.string(mandatory = True),
        "output_name": attr.string(mandatory = True),
        "_tool": attr.label(
            allow_single_file = True,
            cfg = "exec",
            default = Label("//packaging/release:release_tool"),
            executable = True,
        ),
    },
)

"""Deterministic portable archive assembly."""

def _portable_archive_impl(ctx):
    output = ctx.actions.declare_file(ctx.attr.output_name)
    args = ctx.actions.args()
    args.add("c")
    args.add(output.path)
    args.add("artisan-editor/artisan-editor.exe=" + ctx.file.editor.path)
    args.add("artisan-editor/artisan-forge.exe=" + ctx.file.forge.path)
    ctx.actions.run(
        executable = ctx.file._zipper,
        inputs = [ctx.file.editor, ctx.file.forge, ctx.file.layout],
        outputs = [output],
        tools = [ctx.file._zipper],
        arguments = [args],
        mnemonic = "PortableArchive",
        progress_message = "Assembling portable archive %{label}",
    )
    return [DefaultInfo(files = depset([output]))]

portable_archive = rule(
    implementation = _portable_archive_impl,
    attrs = {
        "editor": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "forge": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "layout": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "output_name": attr.string(mandatory = True),
        "_zipper": attr.label(
            default = Label("@bazel_tools//tools/zip:zipper"),
            allow_single_file = True,
            cfg = "exec",
            executable = True,
        ),
    },
)

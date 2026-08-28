"""Bazel-owned Cap'n Proto code generation for the Phase 1 feasibility proof.

This rule makes schema compilation an explicit Bazel action: the upstream
`capnp` compiler and the `capnpc-rust` plugin are declared tool inputs, every
generated `.rs` file is declared up front as an action output, and the action
runs without a shell so the proof stays viable on Windows/MSVC.

The `compiler` and `plugin` attributes are mandatory executable tool inputs.
The rule invokes both by exact execroot path and disables standard imports, so
the action never consults PATH, a shell, or host include directories.

Output contract: for each input `name.capnp` the rule emits exactly one file
`<out_dir>/name_capnp.rs`, matching the upstream `capnpc-rust` naming scheme,
so generated files land next to the crate sources that `mod`-declare them.
"""

def _capnp_codegen_impl(ctx):
    compiler = ctx.executable.compiler
    plugin = ctx.executable.plugin

    srcs = sorted(ctx.files.srcs, key = lambda src: src.path)
    if not srcs:
        fail("capnp_codegen target {label} received no `.capnp` sources.".format(label = ctx.label))

    # One deterministic output per schema, declared before execution so the
    # action graph never depends on compiler-side effects.
    outputs = []
    for src in srcs:
        stem = src.basename[:-len(src.extension) - 1]
        outputs.append(ctx.actions.declare_file("{}/{}_capnp.rs".format(ctx.attr.out_dir, stem)))

    source_dir = srcs[0].dirname
    for src in srcs:
        if src.dirname != source_dir:
            fail("capnp_codegen target {label} requires all schemas to share one source directory.".format(
                label = ctx.label,
            ))

    # The compiler runs from the execroot. The source prefix prevents the
    # workspace-relative schema directory from being reproduced under the
    # declared output directory.
    out_dir_path = outputs[0].dirname

    args = ctx.actions.args()
    args.add("compile")
    args.add("--no-standard-import")
    args.add("--src-prefix={}".format(source_dir))
    args.add("--output={}:{}".format(plugin.path, out_dir_path))
    args.add_all(srcs)

    ctx.actions.run(
        inputs = srcs,
        tools = [compiler, plugin],
        outputs = outputs,
        executable = compiler,
        arguments = [args],
        mnemonic = "CapnpRustCodegen",
        progress_message = "Generating Cap'n Proto Rust bindings for {}".format(
            ", ".join([src.short_path for src in srcs]),
        ),
        use_default_shell_env = False,
    )

    return [DefaultInfo(files = depset(outputs))]

capnp_codegen = rule(
    implementation = _capnp_codegen_impl,
    attrs = {
        "srcs": attr.label_list(
            mandatory = True,
            allow_files = [".capnp"],
            doc = "Cap'n Proto schema files to compile.",
        ),
        "out_dir": attr.string(
            default = "src",
            doc = "Execroot-relative directory (under this package) receiving the generated files.",
        ),
        "compiler": attr.label(
            mandatory = True,
            executable = True,
            cfg = "exec",
            allow_single_file = True,
            doc = "The `capnp` schema compiler executable.",
        ),
        "plugin": attr.label(
            mandatory = True,
            executable = True,
            cfg = "exec",
            allow_single_file = True,
            doc = "The `capnpc-rust` code generator plugin executable.",
        ),
    },
    doc = "Generates Rust bindings from Cap'n Proto schemas as explicit Bazel action outputs.",
)

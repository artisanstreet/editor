"""Deterministic version payload archive assembly."""

_SAFE_MEMBER_CHARS = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-/"
_RESERVED_DEVICES = ["con", "prn", "aux", "nul"]


def _validate_segment(segment, description):
    if segment in ["", ".", ".."]:
        fail("%s contains an empty or dot path segment: %r" % (description, segment))
    if segment.endswith("."):
        fail("%s has a trailing dot: %r" % (description, segment))
    device = segment.split(".", 1)[0].lower()
    if device in _RESERVED_DEVICES or (
        len(device) == 4 and
        device[:3] in ["com", "lpt"] and
        device[3] in "0123456789"
    ):
        fail("%s names a reserved Windows device: %r" % (description, segment))
    if len(segment) > 255:
        fail("%s has a path segment longer than 255 bytes: %r" % (description, segment))


def _validate_member_name(name, description):
    if not name:
        fail("%s must not be empty" % description)
    if len(name) > 65535:
        fail("%s is too long for a ZIP member: %r" % (description, name))
    if name.startswith("/"):
        fail("%s must be relative: %r" % (description, name))
    for index in range(len(name)):
        if name[index] not in _SAFE_MEMBER_CHARS:
            fail("%s contains a non-canonical or non-ASCII character: %r" % (description, name))
    for segment in name.split("/"):
        _validate_segment(segment, description)


def _single_default_file(target, description):
    files = target.files.to_list()
    if len(files) != 1:
        fail(
            "%s must resolve to exactly one Bazel file, found %d" %
            (description, len(files)),
        )
    return files[0]


def _add_member(member_files, member_folds, name, source, description):
    _validate_member_name(name, description)
    if name in member_files:
        fail("duplicate archive member %r" % name)
    folded = name.lower()
    if folded in member_folds:
        fail(
            "ASCII case-folded archive member collision between %r and %r" %
            (member_folds[folded], name),
        )
    member_files[name] = source
    member_folds[folded] = name


def _declared_member(member_files, member_folds, directory, target, suffix, kind):
    for segment in suffix.split("/"):
        _validate_segment(segment, "%s suffix" % kind)
    name = directory + "/" + suffix
    source = _single_default_file(target, "%s input %r" % (kind, suffix))
    _add_member(member_files, member_folds, name, source, "%s member" % kind)


def _versioned_payload_archive_impl(ctx):
    _validate_member_name(ctx.attr.output_name, "output_name")

    member_files = {}
    member_folds = {}
    executable_sources = [
        ("ae", ctx.file.ae),
        ("editor", ctx.file.editor),
        ("forge", ctx.file.forge),
        ("installer", ctx.file.installer),
    ]
    for name, source in executable_sources:
        _add_member(
            member_files,
            member_folds,
            "bin/" + name + ctx.attr.executable_suffix,
            source,
            "executable member",
        )

    for target, suffix in ctx.attr.resources.items():
        _declared_member(member_files, member_folds, "resources", target, suffix, "resource")
    for target, suffix in ctx.attr.licenses.items():
        _declared_member(member_files, member_folds, "licenses", target, suffix, "license")

    members = sorted(member_files.keys())

    output = ctx.actions.declare_file(ctx.attr.output_name)
    manifest = ctx.actions.declare_file(ctx.attr.output_name + ".payload-manifest.json")

    manifest_args = ctx.actions.args()
    manifest_args.add("--output")
    manifest_args.add(manifest.path)
    manifest_args.add("--layout")
    manifest_args.add(ctx.file.layout.path)
    for member in members:
        manifest_args.add("--file")
        manifest_args.add(member)
        manifest_args.add(member_files[member].path)

    declared_inputs = [ctx.file.layout]
    for member in members:
        declared_inputs.append(member_files[member])
    ctx.actions.run(
        executable = ctx.executable._manifest_generator,
        inputs = depset(declared_inputs),
        outputs = [manifest],
        tools = [ctx.executable._manifest_generator],
        arguments = [manifest_args],
        mnemonic = "VersionedPayloadManifest",
        progress_message = "Generating versioned payload manifest %{label}",
    )

    archive_args = ctx.actions.args()
    archive_args.add("c")
    archive_args.add(output.path)
    archive_members = sorted(members + ["payload-manifest.json"])
    for member in archive_members:
        source = manifest if member == "payload-manifest.json" else member_files[member]
        archive_args.add(member + "=" + source.path)

    archive_inputs = [ctx.file.layout, manifest]
    for member in members:
        archive_inputs.append(member_files[member])
    ctx.actions.run(
        executable = ctx.executable._zipper,
        inputs = depset(archive_inputs),
        outputs = [output],
        tools = [ctx.executable._zipper],
        arguments = [archive_args],
        mnemonic = "VersionedPayloadArchive",
        progress_message = "Assembling versioned payload archive %{label}",
    )
    return [DefaultInfo(files = depset([output]))]


versioned_payload_archive = rule(
    implementation = _versioned_payload_archive_impl,
    attrs = {
        "ae": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "editor": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "forge": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "installer": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "layout": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "resources": attr.label_keyed_string_dict(
            allow_files = True,
            default = {},
        ),
        "licenses": attr.label_keyed_string_dict(
            allow_files = True,
            default = {},
        ),
        "executable_suffix": attr.string(mandatory = True),
        "output_name": attr.string(mandatory = True),
        "_manifest_generator": attr.label(
            default = Label(":payload_manifest_generator"),
            allow_single_file = True,
            cfg = "exec",
            executable = True,
        ),
        "_zipper": attr.label(
            default = Label("@bazel_tools//tools/zip:zipper"),
            allow_single_file = True,
            cfg = "exec",
            executable = True,
        ),
    },
)

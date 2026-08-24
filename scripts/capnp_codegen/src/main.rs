//! Bazel tool wrapper for the upstream `capnpc-rust` compiler plugin.

use std::io;
use std::path::Path;

fn main() -> capnp::Result<()> {
    capnpc::codegen::CodeGenerationCommand::new()
        .output_directory(Path::new("."))
        .run(io::stdin())
}

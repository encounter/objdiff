#[cfg(feature = "any-arch")]
mod config_gen;

fn main() {
    #[cfg(feature = "bindings")]
    compile_protos();
    #[cfg(feature = "any-arch")]
    config_gen::generate_diff_config();
}

#[cfg(feature = "bindings")]
fn compile_protos() {
    use std::path::{Path, PathBuf};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("protos");
    let descriptor_path = root.join("proto_descriptor.bin");
    println!("cargo:rerun-if-changed={}", descriptor_path.display());
    let descriptor_mtime = std::fs::metadata(&descriptor_path)
        .map(|m| m.modified().unwrap())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let mut run_protoc = false;
    let proto_files = vec![root.join("diff.proto"), root.join("report.proto")];
    for proto_file in &proto_files {
        println!("cargo:rerun-if-changed={}", proto_file.display());
        let mtime = match std::fs::metadata(proto_file) {
            Ok(m) => m.modified().unwrap(),
            Err(e) => panic!("Failed to stat proto file {}: {:?}", proto_file.display(), e),
        };
        if mtime > descriptor_mtime {
            run_protoc = true;
        }
    }

    fn prost_config(descriptor_path: &Path, run_protoc: bool) -> prost_build::Config {
        let mut config = prost_build::Config::new();
        config.file_descriptor_set_path(descriptor_path);
        // If our cached descriptor is up-to-date, we don't need to run protoc.
        // This is helpful so that users don't need to have protoc installed
        // unless they're updating the protos.
        if !run_protoc {
            config.skip_protoc_run();
        }
        config
    }
    if let Err(e) =
        prost_config(&descriptor_path, run_protoc).compile_protos(&proto_files, &[root.as_path()])
    {
        if e.kind() == std::io::ErrorKind::NotFound && e.to_string().contains("protoc") {
            eprintln!("protoc not found, skipping protobuf compilation");
            prost_config(&descriptor_path, false)
                .compile_protos(&proto_files, &[root.as_path()])
                .expect("Failed to compile protos");
        } else {
            panic!("Failed to compile protos: {e:?}");
        }
    }

    let descriptor_set = std::fs::read(descriptor_path).expect("Failed to read descriptor set");
    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_set)
        .expect("Failed to register descriptors")
        .preserve_proto_field_names()
        .build(&[".objdiff"])
        .expect("Failed to build pbjson");
}

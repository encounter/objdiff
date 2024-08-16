use std::path::PathBuf;

fn main() {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("Failed to execute git");
    let rev = String::from_utf8(output.stdout).expect("Failed to parse git output");
    println!("cargo:rustc-env=GIT_COMMIT_SHA={rev}");
    println!("cargo:rustc-rerun-if-changed=.git/HEAD");

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("protos");
    let proto_files = vec![root.join("report.proto")];

    // Tell cargo to recompile if any of these proto files are changed
    for proto_file in &proto_files {
        println!("cargo:rerun-if-changed={}", proto_file.display());
    }

    let descriptor_path =
        PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("proto_descriptor.bin");

    prost_build::Config::new()
        .file_descriptor_set_path(&descriptor_path)
        .compile_protos(&proto_files, &[root])
        .expect("Failed to compile protos");

    let descriptor_set = std::fs::read(descriptor_path).expect("Failed to read descriptor set");
    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_set)
        .expect("Failed to register descriptors")
        .preserve_proto_field_names()
        .build(&[".objdiff"])
        .expect("Failed to build pbjson");
}

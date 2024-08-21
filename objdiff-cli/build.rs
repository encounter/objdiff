fn main() {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("Failed to execute git");
    let rev = String::from_utf8(output.stdout).expect("Failed to parse git output");
    println!("cargo:rustc-env=GIT_COMMIT_SHA={rev}");
    println!("cargo:rustc-rerun-if-changed=.git/HEAD");
}

//! Exercises the actual Cargo package from an independent application. This
//! opt-in check runs through Cargo in CI; it needs only the existing toolchain.
use std::{fs, path::Path, process::Command};

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Temporary(std::path::PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "builds a separate application; run explicitly in the platform validation job"]
fn an_independent_application_uses_the_packaged_core() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let target = std::env::current_exe().unwrap();
    let target = target.parent().unwrap().parent().unwrap().parent().unwrap();
    let cargo = env!("CARGO");
    run(Command::new(cargo)
        .current_dir(workspace)
        .args([
            "package",
            "-p",
            "nouli-core",
            "--allow-dirty",
            "--no-verify",
            "--locked",
            "--offline",
        ])
        .arg("--target-dir")
        .arg(target));
    let directory = Temporary(std::env::temp_dir().join(format!(
        "nouli-consumer-{}",
        nouli_core::identity::Id::generate()
    )));
    fs::create_dir_all(&directory.0).unwrap();
    let package = target.join(format!(
        "package/nouli-core-{}.crate",
        env!("CARGO_PKG_VERSION")
    ));
    let unpacked = directory.0.join("package");
    tar::Archive::new(flate2::read::GzDecoder::new(
        fs::File::open(package).unwrap(),
    ))
    .unpack(&unpacked)
    .unwrap();
    let packaged = unpacked.join(format!("nouli-core-{}", env!("CARGO_PKG_VERSION")));
    assert!(packaged.join("README.md").is_file());
    // The downstream build verifies the actual archive, rather than compiling
    // the library once in cargo package and again in this independent consumer.
    let consumer = directory.0.join("consumer");
    run(Command::new(cargo)
        .args([
            "init",
            "--bin",
            "--name",
            "nouli-core-consumer",
            "--vcs",
            "none",
        ])
        .arg(&consumer));
    run(Command::new(cargo)
        .current_dir(&consumer)
        .args(["add", "nouli-core", "--offline", "--path"])
        .arg(packaged));
    fs::write(
        consumer.join("src/main.rs"),
        include_str!("support/consumer.rs"),
    )
    .unwrap();
    run(Command::new(cargo)
        .current_dir(&consumer)
        .env("CARGO_TARGET_DIR", target.join("consumer-check"))
        .args(["run", "--offline", "--quiet", "--"])
        .arg(directory.0.join("store")));
}

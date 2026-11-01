#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
use harness::Home;
use std::{
    io::{self, Write},
    process::{Command, Output, Stdio},
};

fn worker(home: &Home, extension: &str, bytes: &[u8]) -> Output {
    // Home::command inserts --config first, but the private worker flag must
    // be argv[1] so the hook runs before normal argument/configuration parsing.
    let mut child = Command::new(env!("CARGO_BIN_EXE_nouli"))
        .args(["--nouli-file-worker", extension])
        .env_clear()
        .env("HOME", &home.0)
        .current_dir(&home.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    if let Err(error) = stdin.write_all(bytes) {
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
    drop(stdin);
    child.wait_with_output().unwrap()
}

#[test]
fn private_worker_renders_svg_without_loading_configuration() {
    let home = Home::create();
    let output = worker(
        &home,
        "svg",
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="3"><rect width="2" height="3" fill="red"/></svg>"#,
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stderr, [] as [u8; 0]);
    assert_eq!(
        image::guess_format(&output.stdout).unwrap(),
        image::ImageFormat::Png
    );
    let image = image::load_from_memory(&output.stdout).unwrap();
    assert_eq!((image.width(), image.height()), (2, 3));
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);
}

#[test]
fn private_worker_rejects_unknown_extensions_and_oversized_input() {
    let home = Home::create();
    let oversized = vec![b' '; 16 * 1024 * 1024 + 1];
    for (extension, input) in [("not-a-document", &b""[..]), ("svg", oversized.as_slice())] {
        let output = worker(&home, extension, input);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stdout, [] as [u8; 0]);
        assert_eq!(output.stderr, [] as [u8; 0]);
    }
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);
}

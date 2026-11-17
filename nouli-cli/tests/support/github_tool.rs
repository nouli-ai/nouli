//! Offline GitHub responses for the release controller's Cargo regression tests.
use std::{fs, io, io::Write, path::PathBuf};

pub fn run(args: &[String]) -> Option<io::Result<()>> {
    let program = std::env::args_os().next()?;
    let name = std::path::Path::new(&program).file_name()?.to_str()?;
    if name == "sleep" {
        return Some(Ok(()));
    }
    if name != "gh" {
        return None;
    }
    Some(respond(args))
}

fn respond(args: &[String]) -> io::Result<()> {
    let root = PathBuf::from(std::env::var_os("FIXTURE_GITHUB").unwrap());
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("calls"))?;
    writeln!(log, "{}", serde_json::to_string(args)?)?;
    if std::env::var_os("FIXTURE_API_FAILURE").is_some() {
        return Err(io::Error::other("fixture API failure"));
    }
    if args.first().is_some_and(|arg| arg == "workflow") {
        return fs::write(root.join("dispatched"), serde_json::to_vec(args)?);
    }
    if args.iter().any(|arg| arg == "--jq") {
        return io::stdout().write_all(b"https://example.invalid/failed-validation\n");
    }
    let kind = if args.get(1).unwrap().contains("/artifacts") {
        "catalog"
    } else {
        "runs"
    };
    let stage = if root.join("dispatched").exists() {
        "after"
    } else {
        "before"
    };
    io::stdout().write_all(&fs::read(root.join(format!("{kind}-{stage}.json")))?)
}

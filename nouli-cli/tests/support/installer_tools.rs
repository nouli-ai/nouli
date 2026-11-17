//! Rust stand-ins for the Unix installer's host discovery and HTTPS downloads.
use std::{fs, io, io::Write};

pub fn run(args: &[String]) -> Option<io::Result<()>> {
    let program = std::env::args_os().next()?;
    let name = std::path::Path::new(&program).file_name()?.to_str()?;
    let result = match name {
        "uname" => emit(if args.first().is_some_and(|a| a == "-s") {
            "FIXTURE_SYSTEM"
        } else {
            "FIXTURE_ARCH"
        }),
        "getconf" => emit("FIXTURE_LIBC"),
        "curl" => download(args),
        _ => return None,
    };
    Some(result)
}

fn emit(variable: &str) -> io::Result<()> {
    io::stdout().write_all(std::env::var(variable).unwrap_or_default().as_bytes())
}

fn download(args: &[String]) -> io::Result<()> {
    let url = args.iter().find(|a| a.starts_with("https://")).unwrap();
    let mut requests = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::var_os("FIXTURE_REQUESTS").unwrap())?;
    writeln!(requests, "{url}")?;
    if std::env::var_os("FIXTURE_DOWNLOAD_FAILURE").is_some() {
        return Err(io::Error::other("fixture download failure"));
    }
    if url.ends_with("/latest") {
        return io::stdout().write_all(b"https://github.com/nouli-ai/nouli/releases/tag/v9.8.7");
    }
    let destination = &args[args.iter().position(|a| a == "-o").unwrap() + 1];
    if url.ends_with(".sha256") {
        fs::write(
            destination,
            format!(
                "{}  {}\n",
                std::env::var("FIXTURE_CHECKSUM").unwrap(),
                url.rsplit('/').next().unwrap().trim_end_matches(".sha256")
            ),
        )
    } else {
        fs::copy(std::env::var_os("FIXTURE_ARCHIVE").unwrap(), destination).map(|_| ())
    }
}

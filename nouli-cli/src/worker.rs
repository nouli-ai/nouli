//! Private document-parser entry point and its bounded subprocess client.
use crate::{
    process::{Limits, run_bounded},
    repl::tui::attachments::{Error, MAX_BYTES, documents},
};
use std::{
    io::{Read, Write},
    process::{Command, ExitCode, Stdio},
    time::Duration,
};

pub(crate) const WORKER_EXTENSIONS: &[&str] =
    &["svg", "docx", "pptx", "xls", "xlsx", "xlsm", "xlsb", "ods"];
const FLAG: &str = "--nouli-file-worker";

pub(crate) fn maybe_worker() -> Option<ExitCode> {
    let mut args = std::env::args();
    if args.nth(1).as_deref() != Some(FLAG) {
        return None;
    }
    let result = (|| {
        Limits::Parser.apply().ok()?;
        let extension = args.next()?;
        if !WORKER_EXTENSIONS.contains(&extension.as_str()) {
            return None;
        }
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_BYTES {
            return None;
        }
        let output = if extension == "svg" {
            documents::svg(&bytes, "file").ok()?
        } else {
            documents::text(bytes, &extension, "file")
                .ok()?
                .into_bytes()
        };
        if output.len() as u64 > MAX_BYTES {
            return None;
        }
        std::io::stdout().write_all(&output).ok()
    })();
    Some(if result.is_some() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
}

pub(crate) fn extract(bytes: &[u8], extension: &str, name: &str) -> Result<Vec<u8>, Error> {
    let fail = || Error::Unsupported(name.into());
    let mut command = Command::new(std::env::current_exe().map_err(|_| fail())?);
    command.args([FLAG, extension]).stdout(Stdio::piped());
    run_bounded(
        &mut command,
        Some(bytes),
        Duration::from_secs(10),
        MAX_BYTES,
        None,
        Some(Limits::Parser),
    )
    .map_err(|_| fail())
}

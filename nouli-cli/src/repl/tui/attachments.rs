//! Terminal file drops are paths, either bracketed paste or ordinary key input.
//! Resolve shell quoting without executing it, and snapshot bytes at drop time.
mod documents;
mod files;
mod worker;
pub(super) use files::discover;
use std::{
    fs::File,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};
pub(crate) use worker::maybe_worker;

use base64::{Engine, prelude::BASE64_STANDARD};
use nouli_core::{
    context::{
        ContextUpdate,
        evidence::{self, marker},
    },
    identity::Id,
    media::Image,
};
use thiserror::Error;

const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub(super) enum Error {
    #[error("Cannot read {0}.")]
    Read(String),
    #[error("{0} is larger than 16 MiB. Choose a smaller file.")]
    Large(String),
    #[error("Cannot extract {0}. Choose a readable text file, document, spreadsheet, or image.")]
    Unsupported(String),
    #[error("{0} import needs LibreOffice. Install it or export as DOCX/PPTX first.")]
    OfficeUnavailable(String),
    #[error("Cannot render {0}. Embed linked assets in the SVG or export it as PNG.")]
    Svg(String),
    #[error("PDF import needs pdftotext (Poppler). Install it, then drop the file again.")]
    PdfUnavailable,
}

pub(super) struct Loaded {
    pub text: String,
    pub image: Option<Image>,
}

/// Recognize only complete path drops. Ordinary prose remains literal.
pub(super) fn paths(text: &str) -> Option<Vec<PathBuf>> {
    fn path(text: &str) -> Option<PathBuf> {
        let text = text.trim();
        if text.starts_with("file://") {
            return url::Url::parse(text).ok()?.to_file_path().ok();
        }
        if let Some(tail) = text.strip_prefix("~/") {
            return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(tail));
        }
        Some(PathBuf::from(text))
    }
    let whole = path(text)?;
    if whole.is_file() {
        return Some(vec![whole]);
    }
    let tokens = shlex::split(text)?;
    if tokens.is_empty() {
        return None;
    }
    let paths: Vec<_> = tokens
        .iter()
        .map(|text| path(text))
        .collect::<Option<_>>()?;
    paths.iter().all(|path| path.is_file()).then_some(paths)
}

pub(super) fn load(path: &Path) -> Result<Loaded, Error> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let name = crate::render::clean(&name);
    let file = File::open(path).map_err(|_| Error::Read(name.clone()))?;
    if file
        .metadata()
        .map_err(|_| Error::Read(name.clone()))?
        .len()
        > MAX_BYTES
    {
        return Err(Error::Large(name));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Read(name.clone()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::Large(name));
    }
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if extension == "svg" {
        bytes = worker::extract(bytes, "svg", &name).map_err(|_| Error::Svg(name.clone()))?;
    }
    if let Ok(format) = image::guess_format(&bytes) {
        let reader = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|_| Error::Unsupported(name.clone()))?;
        let image = reader
            .decode()
            .map_err(|_| Error::Unsupported(name.clone()))?;
        // Preserve compressed JPEG/PNG snapshots; normalize other formats.
        let (format, bytes) = match format {
            image::ImageFormat::Png => ("png", bytes),
            image::ImageFormat::Jpeg => ("jpeg", bytes),
            _ => {
                let mut png = Cursor::new(Vec::new());
                image
                    .write_to(&mut png, image::ImageFormat::Png)
                    .map_err(|_| Error::Unsupported(name.clone()))?;
                ("png", png.into_inner())
            }
        };
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Error::Large(name));
        }
        // References must survive textual serialization unchanged. Filenames
        // can contain quotes/backslashes, so keep them outside the opaque token.
        let reference = marker::image_marker(Id::generate());
        return Ok(Loaded {
            text: format!("{name} {reference}"),
            image: Some(Image {
                reference,
                name,
                format: format.into(),
                data_base64: BASE64_STANDARD.encode(bytes),
            }),
        });
    }
    let text = if bytes.starts_with(b"%PDF-") {
        pdf_text(bytes, &name)?
    } else if matches!(extension.as_str(), "csv" | "tsv") {
        documents::delimited(&bytes, if extension == "tsv" { b'\t' } else { b',' }, &name)?
    } else if matches!(
        extension.as_str(),
        "docx" | "pptx" | "xls" | "xlsx" | "xlsm" | "xlsb" | "ods" | "doc" | "ppt"
    ) {
        if matches!(extension.as_str(), "doc" | "ppt") {
            documents::text(bytes, &extension, &name)?
        } else {
            String::from_utf8(worker::extract(bytes, &extension, &name)?)
                .map_err(|_| Error::Unsupported(name.clone()))?
        }
    } else {
        String::from_utf8(bytes).map_err(|_| Error::Unsupported(name.clone()))?
    };
    if text.trim().is_empty()
        || text.len() as u64 > MAX_BYTES
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(Error::Unsupported(name));
    }
    Ok(Loaded { text, image: None })
}

/// Keep document parsing outside the TUI process, with bounded output and time.
fn pdf_text(bytes: Vec<u8>, name: &str) -> Result<String, Error> {
    use std::{
        io::Write,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    let mut child = Command::new("pdftotext")
        .args(["-layout", "-nopgbrk", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::PdfUnavailable
            } else {
                Error::Read(name.into())
            }
        })?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let writer = thread::spawn(move || stdin.write_all(&bytes));
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take(MAX_BYTES + 1)
            .read_to_end(&mut output)
            .map(|_| output)
    });
    let start = Instant::now();
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if start.elapsed() < Duration::from_secs(10) => {
                thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let _ = writer.join();
    let output = reader
        .join()
        .ok()
        .and_then(Result::ok)
        .filter(|output| output.len() as u64 <= MAX_BYTES);
    if !success {
        return Err(Error::Unsupported(name.into()));
    }
    String::from_utf8(output.ok_or_else(|| Error::Unsupported(name.into()))?)
        .map_err(|_| Error::Unsupported(name.into()))
}

pub(super) fn update(image: &Image) -> ContextUpdate {
    ContextUpdate::Append {
        record: evidence::image_record(Id::generate(), image),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_accept_escaped_quoted_unicode_paths_and_file_urls_without_shell_execution() {
        let dir = std::env::temp_dir().join(format!("nouli-drop-{}", Id::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a 中文 $(echo nope).txt");
        std::fs::write(&path, "What now?\nKeep, [this] <literal>.").unwrap();
        for text in [
            path.display().to_string(),
            shlex::try_quote(path.to_str().unwrap())
                .unwrap()
                .into_owned(),
            url::Url::from_file_path(&path).unwrap().to_string(),
        ] {
            assert_eq!(paths(&text), Some(vec![path.clone()]));
        }
        assert!(paths("ordinary prose").is_none());
        assert!(load(&path).unwrap().text.contains("[this]"));
        let png = dir.join("photo.png");
        image::RgbImage::new(2, 2).save(&png).unwrap();
        let image = load(&png).unwrap().image.unwrap();
        assert_eq!(
            BASE64_STANDARD.decode(&image.data_base64).unwrap(),
            std::fs::read(&png).unwrap()
        );
        assert_eq!(image.name, "photo.png");
        let saved = update(&image).new_record().unwrap().as_image().unwrap();
        assert_eq!(saved.unwrap().data_base64, image.data_base64);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_pdfs_and_oversized_files_return_errors_without_panicking() {
        let path = std::env::temp_dir().join(format!("nouli-invalid-pdf-{}", Id::generate()));
        std::fs::write(&path, b"%PDF-1.4\ninvalid").unwrap();
        assert!(load(&path).is_err());
        File::create(&path).unwrap().set_len(MAX_BYTES + 1).unwrap();
        assert!(matches!(load(&path), Err(Error::Large(_))));
        std::fs::remove_file(path).unwrap();
    }
}

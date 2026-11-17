//! Terminal file drops are paths, either bracketed paste or ordinary key input.
//! Resolve shell quoting without executing it, and snapshot bytes at drop time.
pub(crate) mod documents;
mod files;
use crate::worker;
pub(super) use files::discover;
use std::{
    fs::File,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

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

pub(crate) const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub(crate) enum Error {
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

#[cfg(test)]
pub(super) fn quote_drop(path: &Path) -> String {
    #[cfg(unix)]
    {
        shlex::try_quote(path.to_str().unwrap())
            .unwrap()
            .into_owned()
    }
    #[cfg(windows)]
    {
        format!("\"{}\"", path.display())
    }
}

/// Windows file drops preserve backslashes; PowerShell may prefix a quoted
/// filename with its call operator, which is parsed as data and never executed.
#[cfg(any(windows, test))]
fn windows_path_tokens(text: &str) -> Option<Vec<String>> {
    let text = text.trim().strip_prefix("& ").unwrap_or(text.trim());
    let mut chars = text.chars().peekable();
    let mut paths = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    while let Some(character) = chars.next() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                if delimiter == '\'' && chars.peek() == Some(&'\'') {
                    token.push('\'');
                    chars.next();
                } else {
                    quote = None;
                }
            } else {
                token.push(character);
            }
        } else if (character == '\'' || character == '"') && token.is_empty() {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !token.is_empty() {
                paths.push(std::mem::take(&mut token));
            }
        } else {
            token.push(character);
        }
    }
    if quote.is_some() {
        return None;
    }
    if !token.is_empty() {
        paths.push(token);
    }
    Some(paths)
}

/// Recognize only complete path drops. Ordinary prose remains literal.
pub(super) fn paths(text: &str) -> Option<Vec<PathBuf>> {
    fn path(text: &str) -> Option<PathBuf> {
        let text = text.trim();
        if text.starts_with("file://") {
            return url::Url::parse(text).ok()?.to_file_path().ok();
        }
        if let Some(tail) = text.strip_prefix("~/") {
            return crate::settings::home().map(|home| home.join(tail));
        }
        Some(PathBuf::from(text))
    }
    let whole = path(text)?;
    if whole.is_file() {
        return Some(vec![whole]);
    }
    #[cfg(unix)]
    let tokens = shlex::split(text)?;
    #[cfg(windows)]
    let tokens = windows_path_tokens(text)?;
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
        bytes = worker::extract(&bytes, "svg", &name).map_err(|_| Error::Svg(name.clone()))?;
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
        pdf_text(&bytes, &name)?
    } else if matches!(extension.as_str(), "csv" | "tsv") {
        documents::delimited(&bytes, if extension == "tsv" { b'\t' } else { b',' }, &name)?
    } else if (worker::WORKER_EXTENSIONS.contains(&extension.as_str()) && extension != "svg")
        || matches!(extension.as_str(), "doc" | "ppt")
    {
        if matches!(extension.as_str(), "doc" | "ppt") {
            documents::text(bytes, &extension, &name)?
        } else {
            String::from_utf8(worker::extract(&bytes, &extension, &name)?)
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

/// Keep PDF conversion outside the TUI, with bounded input, output, and time.
fn pdf_text(bytes: &[u8], name: &str) -> Result<String, Error> {
    pdf_text_with(
        std::process::Command::new("pdftotext"),
        bytes,
        name,
        std::time::Duration::from_secs(10),
    )
}

fn pdf_text_with(
    mut command: std::process::Command,
    bytes: &[u8],
    name: &str,
    timeout: std::time::Duration,
) -> Result<String, Error> {
    use crate::process::{RunError, run_bounded};
    command
        .args(["-layout", "-nopgbrk", "-", "-"])
        .stdout(std::process::Stdio::piped());
    let output = run_bounded(&mut command, Some(bytes), timeout, MAX_BYTES, None, None).map_err(
        |error| match error {
            RunError::Spawn(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Error::PdfUnavailable
            }
            RunError::Spawn(_) => Error::Read(name.into()),
            _ => Error::Unsupported(name.into()),
        },
    )?;
    String::from_utf8(output).map_err(|_| Error::Unsupported(name.into()))
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
    fn missing_pdf_converter_is_actionable() {
        let missing = std::env::temp_dir().join(format!("nouli-missing-{}.exe", Id::generate()));
        assert!(matches!(
            pdf_text_with(
                std::process::Command::new(missing),
                b"%PDF-fixture",
                "fixture.pdf",
                std::time::Duration::from_secs(1)
            ),
            Err(Error::PdfUnavailable)
        ));
    }

    #[test]
    #[ignore = "requires pdftotext on PATH; see docs/windows.md"]
    fn installed_poppler_extracts_pdf_text() {
        use std::fmt::Write as _;
        let content = "BT /F1 12 Tf 20 100 Td (Nouli PDF fixture) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            write!(pdf, "{} 0 obj\n{object}\nendobj\n", index + 1).unwrap();
        }
        let xref = pdf.len();
        pdf.push_str("xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            writeln!(pdf, "{offset:010} 00000 n ").unwrap();
        }
        write!(
            pdf,
            "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .unwrap();
        assert!(
            pdf_text(pdf.as_bytes(), "fixture.pdf")
                .unwrap()
                .contains("Nouli PDF fixture")
        );
    }
    #[test]
    fn windows_drop_quoting_preserves_backslashes_and_apostrophes() {
        assert_eq!(
            windows_path_tokens("& 'C:\\Users\\O''Brien\\旅行.csv' \"D:\\two words.png\""),
            Some(vec![
                "C:\\Users\\O'Brien\\旅行.csv".into(),
                "D:\\two words.png".into()
            ])
        );
        assert_eq!(windows_path_tokens("\"C:\\unfinished"), None);
    }

    #[test]
    fn drops_accept_escaped_quoted_unicode_paths_and_file_urls_without_shell_execution() {
        let dir = std::env::temp_dir().join(format!("nouli-drop-{}", Id::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a 中文 $(echo nope).txt");
        std::fs::write(&path, "What now?\nKeep, [this] <literal>.").unwrap();
        for text in [
            path.display().to_string(),
            quote_drop(&path),
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
    #[test]
    fn pdf_timeout_kills_descendants_after_the_converter_exits() {
        let root = std::env::temp_dir().join(format!(
            "nouli-pdf-timeout-{}",
            nouli_core::identity::Id::generate()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let pidfile = root.join("pid");
        let mut command = crate::test_child::command("spawn");
        command.arg(&pidfile).arg("exit");
        let result = pdf_text_with(
            command,
            b"%PDF-fixture",
            "fixture.pdf",
            std::time::Duration::from_millis(500),
        );
        assert!(result.is_err());
        let pid: u32 = std::fs::read_to_string(&pidfile).unwrap().parse().unwrap();
        for _ in 0..100 {
            if !crate::test_child::running(pid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!crate::test_child::running(pid));
        std::fs::remove_dir_all(root).unwrap();
    }
}

//! Local, read-only extraction. Models receive text/tables or a rendered SVG,
//! never an Office binary disguised as text. No page rendering or OCR.
use super::{Error, MAX_BYTES};
use calamine::Reader;
use std::{
    collections::HashMap,
    fmt::Write,
    io::{Cursor, Read},
};

fn unsupported(name: &str) -> Error {
    Error::Unsupported(name.into())
}

pub(super) fn delimited(bytes: &[u8], separator: u8, name: &str) -> Result<String, Error> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(separator)
        .has_headers(false)
        .flexible(true)
        .from_reader(bytes);
    let mut out = String::new();
    for row in reader.records() {
        let row = row.map_err(|_| unsupported(name))?;
        out.push_str(
            &serde_json::to_string(&row.iter().collect::<Vec<_>>())
                .expect("cell strings serialize"),
        );
        out.push('\n');
        if out.len() as u64 > MAX_BYTES {
            return Err(Error::Large(name.into()));
        }
    }
    Ok(out)
}

fn cell_value(cell: &calamine::Data) -> String {
    if let calamine::Data::DateTime(date) = cell
        && date.is_datetime()
    {
        let (year, month, day, hour, min, sec, milli) = date.to_ymd_hms_milli();
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{milli:03}")
    } else if let calamine::Data::DateTime(date) = cell {
        format!("{} seconds (duration)", date.as_f64() * 86_400.0)
    } else {
        cell.to_string()
    }
}

fn archive<'a>(bytes: &'a [u8], name: &str) -> Result<zip::ZipArchive<Cursor<&'a [u8]>>, Error> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| unsupported(name))?;
    let mut total = 0u64;
    for index in 0..archive.len() {
        total = total.saturating_add(
            archive
                .by_index(index)
                .map_err(|_| unsupported(name))?
                .size(),
        );
        if total > MAX_BYTES {
            return Err(Error::Large(name.into()));
        }
    }
    Ok(archive)
}

fn xml(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    entry: &str,
    name: &str,
) -> Result<String, Error> {
    let mut text = String::new();
    archive
        .by_name(entry)
        .map_err(|_| unsupported(name))?
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| unsupported(name))?;
    if text.len() as u64 > MAX_BYTES {
        return Err(Error::Large(name.into()));
    }
    Ok(text)
}

fn office_text(xml: &str, name: &str) -> Result<String, Error> {
    let doc = roxmltree::Document::parse(xml).map_err(|_| unsupported(name))?;
    let mut out = String::new();
    fn visit(node: roxmltree::Node<'_, '_>, out: &mut String, in_cell: bool, depth: usize) -> bool {
        if depth > 128 {
            return false;
        }
        let tag = node.tag_name().name();
        if tag == "tbl" {
            for row in node.children().filter(|n| n.has_tag_name("tr")) {
                let mut cells = Vec::new();
                for cell in row.children().filter(|n| n.has_tag_name("tc")) {
                    let mut value = String::new();
                    if !visit(cell, &mut value, false, depth + 1) {
                        return false;
                    }
                    cells.push(value.trim_end_matches(['\n', '\t', ' ']).to_owned());
                }
                out.push_str(&serde_json::to_string(&cells).expect("cell strings serialize"));
                out.push('\n');
            }
            return out.len() as u64 <= MAX_BYTES;
        }
        match tag {
            "t" => {
                for text in node.children().filter(|n| n.is_text()) {
                    out.push_str(text.text().unwrap_or_default());
                }
            }
            "tab" => out.push('\t'),
            "br" => out.push('\n'),
            _ => {
                for child in node.children().filter(|n| n.is_element()) {
                    if !visit(child, out, in_cell || tag == "tc", depth + 1) {
                        return false;
                    }
                }
                match tag {
                    "p" => out.push(if in_cell { ' ' } else { '\n' }),
                    "tc" => {
                        out.push('\t');
                    }
                    "tr" => {
                        if out.ends_with('\t') {
                            out.pop();
                        }
                        out.push('\n');
                    }
                    _ => {}
                }
            }
        }
        out.len() as u64 <= MAX_BYTES
    }
    if !visit(doc.root_element(), &mut out, false, 0) {
        return Err(unsupported(name));
    }
    Ok(out)
}

pub(super) fn text(bytes: Vec<u8>, extension: &str, name: &str) -> Result<String, Error> {
    if matches!(extension, "doc" | "ppt") {
        return legacy(bytes, extension, name);
    }
    // Bound decompression for all ZIP-based document formats before parsing.
    if bytes.starts_with(b"PK") {
        archive(&bytes, name)?;
    }
    if matches!(extension, "xls" | "xlsx" | "xlsm" | "xlsb" | "ods") {
        let mut book = calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
            .map_err(|_| unsupported(name))?;
        let mut out = String::new();
        for sheet in book.sheet_names().to_vec() {
            let range = book
                .worksheet_range(&sheet)
                .map_err(|_| unsupported(name))?;
            let formulas = book
                .worksheet_formula(&sheet)
                .map_err(|_| unsupported(name))?;
            let starts = [range.start(), formulas.start()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            let ends = [range.end(), formulas.end()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            writeln!(out, "Sheet: {sheet}").expect("string write");
            if starts.is_empty() {
                continue;
            }
            let (first_row, first_col) = (
                starts.iter().map(|p| p.0).min().unwrap(),
                starts.iter().map(|p| p.1).min().unwrap(),
            );
            let (last_row, last_col) = (
                ends.iter().map(|p| p.0).max().unwrap(),
                ends.iter().map(|p| p.1).max().unwrap(),
            );
            if u64::from(last_row - first_row + 1) * u64::from(last_col - first_col + 1) > 1_000_000
            {
                return Err(Error::Large(name.into()));
            }
            for row in first_row..=last_row {
                let mut cells = Vec::new();
                for col in first_col..=last_col {
                    let value = match range.get_value((row, col)) {
                        Some(cell) if !matches!(cell, calamine::Data::Empty) => cell_value(cell),
                        _ => formulas
                            .get_value((row, col))
                            .filter(|s| !s.is_empty())
                            .map(|formula| format!("={formula} (no cached result)"))
                            .unwrap_or_default(),
                    };
                    cells.push(value);
                }
                out.push_str(&serde_json::to_string(&cells).expect("cell strings serialize"));
                out.push('\n');
                if out.len() as u64 > MAX_BYTES {
                    return Err(Error::Large(name.into()));
                }
            }
        }
        return Ok(out);
    }
    let mut archive = archive(&bytes, name)?;
    if extension == "docx" {
        return office_text(&xml(&mut archive, "word/document.xml", name)?, name);
    }
    let presentation = xml(&mut archive, "ppt/presentation.xml", name)?;
    let relationships = xml(&mut archive, "ppt/_rels/presentation.xml.rels", name)?;
    let doc = roxmltree::Document::parse(&presentation).map_err(|_| unsupported(name))?;
    let rels = roxmltree::Document::parse(&relationships).map_err(|_| unsupported(name))?;
    let targets = rels
        .descendants()
        .filter_map(|node| {
            if node.attribute("TargetMode") == Some("External") {
                return None;
            }
            Some((node.attribute("Id")?, node.attribute("Target")?))
        })
        .collect::<HashMap<_, _>>();
    let mut out = String::new();
    for (index, slide) in doc
        .descendants()
        .filter(|node| node.has_tag_name("sldId"))
        .enumerate()
    {
        let id = slide
            .attributes()
            .find(|attr| attr.name() == "id" && attr.namespace().is_some())
            .ok_or_else(|| unsupported(name))?
            .value();
        let target = targets.get(id).ok_or_else(|| unsupported(name))?;
        let entry = if target.starts_with('/') {
            target.trim_start_matches('/').to_string()
        } else {
            format!("ppt/{target}")
        };
        writeln!(out, "Slide {}", index + 1).expect("string write");
        out.push_str(&office_text(&xml(&mut archive, &entry, name)?, name)?);
        if out.len() as u64 > MAX_BYTES {
            return Err(Error::Large(name.into()));
        }
    }
    Ok(out)
}

pub(super) fn svg(bytes: &[u8], name: &str) -> Result<Vec<u8>, Error> {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let mut options = resvg::usvg::Options::default();
    // A selected SVG must not pull unrelated local files into model input.
    let unresolved = Arc::new(AtomicBool::new(false));
    let seen = unresolved.clone();
    options.image_href_resolver.resolve_string = Box::new(move |_, _| {
        seen.store(true, Ordering::Relaxed);
        None
    });
    options.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(|_| unsupported(name))?;
    if unresolved.load(Ordering::Relaxed) {
        return Err(Error::Svg(name.into()));
    }
    let size = tree.size();
    let scale = (1568.0 / size.width().max(size.height())).min(1.0);
    let mut pixels = resvg::tiny_skia::Pixmap::new(
        (size.width() * scale).ceil().max(1.0) as u32,
        (size.height() * scale).ceil().max(1.0) as u32,
    )
    .ok_or_else(|| unsupported(name))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixels.as_mut(),
    );
    pixels.encode_png().map_err(|_| unsupported(name))
}

/// Optional converter runs in its own temporary directory and user profile.
fn legacy(bytes: Vec<u8>, extension: &str, name: &str) -> Result<String, Error> {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let temp = Temp(std::env::temp_dir().join(format!(
        "nouli-office-{}",
        nouli_core::identity::Id::generate()
    )));
    use nix::{
        sys::{
            resource::{Resource, setrlimit},
            signal::{Signal, killpg},
        },
        unistd::Pid,
    };
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::process::CommandExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&temp.0)
        .map_err(|_| Error::Read(name.into()))?;
    let input = temp.0.join(format!("input.{extension}"));
    std::fs::write(&input, bytes).map_err(|_| Error::Read(name.into()))?;
    let output = if extension == "doc" { "docx" } else { "pptx" };
    let profile =
        url::Url::from_directory_path(temp.0.join("profile")).map_err(|_| unsupported(name))?;
    let mut process = [
        "soffice",
        "libreoffice",
        "/Applications/LibreOffice.app/Contents/MacOS/soffice",
    ]
    .into_iter()
    .find_map(|program| {
        let mut command = Command::new(program);
        command
            .arg(format!("-env:UserInstallation={profile}"))
            .args(["--headless", "--convert-to", output, "--outdir"])
            .arg(&temp.0)
            .arg(&input)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        // SAFETY: only async-signal-safe setrlimit calls run between fork/exec.
        unsafe {
            command.pre_exec(|| {
                for (kind, limit) in [
                    (Resource::RLIMIT_CORE, 0),
                    (Resource::RLIMIT_CPU, 8),
                    (Resource::RLIMIT_FSIZE, MAX_BYTES),
                ] {
                    setrlimit(kind, limit, limit).map_err(std::io::Error::from)?;
                }
                #[cfg(not(target_os = "macos"))]
                {
                    setrlimit(Resource::RLIMIT_DATA, 512 * 1024 * 1024, 512 * 1024 * 1024)
                        .map_err(std::io::Error::from)?;
                    setrlimit(Resource::RLIMIT_AS, 512 * 1024 * 1024, 512 * 1024 * 1024)
                        .map_err(std::io::Error::from)?;
                }
                Ok(())
            });
        }
        command.spawn().ok()
    })
    .ok_or_else(|| Error::OfficeUnavailable(name.into()))?;
    let start = Instant::now();
    loop {
        match process.try_wait() {
            Ok(Some(status)) if status.success() => {
                let _ = killpg(Pid::from_raw(process.id() as i32), Signal::SIGKILL);
                break;
            }
            Ok(None)
                if start.elapsed() < Duration::from_secs(10)
                    && !super::worker::group_over_memory(process.id()) =>
            {
                thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = killpg(Pid::from_raw(process.id() as i32), Signal::SIGKILL);
                let _ = process.kill();
                let _ = process.wait();
                return Err(unsupported(name));
            }
        }
    }
    Ok(super::load(&temp.0.join(format!("input.{output}")))?.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zipped(files: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn office_extraction_preserves_unicode_entities_and_table_cells() {
        let input = r#"<document><body><p><r><t>旅行 &amp; budget</t><t> €250</t></r></p><tbl><tr><tc><p><t>City</t></p></tc><tc><p><t>Fare</t></p></tc></tr><tr><tc><p><t>Seattle</t></p></tc><tc><p><t>42</t></p></tc></tr></tbl></body></document>"#;
        let bytes = zipped(&[("word/document.xml", input)]);
        let result = text(bytes, "docx", "trip.docx").unwrap();
        assert_eq!(
            result,
            "旅行 & budget €250\n[\"City\",\"Fare\"]\n[\"Seattle\",\"42\"]\n"
        );
    }

    #[test]
    fn slide_order_comes_from_the_presentation_not_filenames() {
        let files = [
            (
                "ppt/presentation.xml",
                r#"<p:presentation xmlns:p="p" xmlns:r="r"><p:sldIdLst><p:sldId id="1" r:id="second"/><p:sldId id="2" r:id="first"/></p:sldIdLst></p:presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<Relationships><Relationship Id="first" Target="slides/slide1.xml"/><Relationship Id="second" Target="slides/slide2.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/slide1.xml",
                "<slide><p><t>First file</t></p></slide>",
            ),
            (
                "ppt/slides/slide2.xml",
                "<slide><p><t>First displayed</t></p></slide>",
            ),
        ];
        assert_eq!(
            text(zipped(&files), "pptx", "deck.pptx").unwrap(),
            "Slide 1\nFirst displayed\nSlide 2\nFirst file\n"
        );
    }

    #[test]
    fn malformed_documents_fail_and_svg_becomes_bounded_pixels() {
        assert!(text(b"not a document".to_vec(), "docx", "bad.docx").is_err());
        let bytes = svg(br#"<svg xmlns="http://www.w3.org/2000/svg" width="4000" height="2000"><rect width="4000" height="2000" fill="red"/></svg>"#, "chart.svg").unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (1568, 784));
        assert_eq!(image.to_rgba8().get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert!(matches!(svg(br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><image href="missing.png" width="20" height="20"/></svg>"#, "linked.svg"), Err(Error::Svg(_))));
    }

    #[test]
    fn table_cells_preserve_delimiters_and_excel_dates_keep_their_meaning() {
        let text = delimited(
            b"name\tnote\nSeattle\t\"line one\nline two\t\"\"quoted\"\"\"\n",
            b'\t',
            "table.tsv",
        )
        .unwrap();
        let rows = text
            .lines()
            .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows[1], vec!["Seattle", "line one\nline two\t\"quoted\""]);
        let date =
            calamine::ExcelDateTime::new(45943.5, calamine::ExcelDateTimeType::DateTime, false);
        assert_eq!(
            cell_value(&calamine::Data::DateTime(date)),
            "2025-10-13T12:00:00.000"
        );
        let duration =
            calamine::ExcelDateTime::new(0.0625, calamine::ExcelDateTimeType::TimeDelta, false);
        assert_eq!(
            cell_value(&calamine::Data::DateTime(duration)),
            "5400 seconds (duration)"
        );
    }
}

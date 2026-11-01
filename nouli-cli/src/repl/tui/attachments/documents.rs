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
                for text in node.children().filter(roxmltree::Node::is_text) {
                    out.push_str(text.text().unwrap_or_default());
                }
            }
            "tab" => out.push('\t'),
            "br" => out.push('\n'),
            _ => {
                for child in node.children().filter(roxmltree::Node::is_element) {
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
    let doc = roxmltree::Document::parse(xml).map_err(|_| unsupported(name))?;
    let mut out = String::new();
    if !visit(doc.root_element(), &mut out, false, 0) {
        return Err(unsupported(name));
    }
    Ok(out)
}

pub(crate) fn text(bytes: Vec<u8>, extension: &str, name: &str) -> Result<String, Error> {
    if matches!(extension, "doc" | "ppt") {
        return legacy(bytes, extension, name);
    }
    // Bound decompression for all ZIP-based document formats before parsing.
    if bytes.starts_with(b"PK") {
        archive(&bytes, name)?;
    }
    if matches!(extension, "xls" | "xlsx" | "xlsm" | "xlsb" | "ods") {
        return spreadsheet(bytes, name);
    }
    let mut archive = archive(&bytes, name)?;
    if extension == "docx" {
        return office_text(&xml(&mut archive, "word/document.xml", name)?, name);
    }
    presentation(&mut archive, name)
}

/// Every sheet as a `Sheet: <name>` header followed by one JSON row per line.
fn spreadsheet(bytes: Vec<u8>, name: &str) -> Result<String, Error> {
    let mut book =
        calamine::open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|_| unsupported(name))?;
    let mut out = String::new();
    for sheet in book.sheet_names().clone() {
        let range = book
            .worksheet_range(&sheet)
            .map_err(|_| unsupported(name))?;
        let formulas = book
            .worksheet_formula(&sheet)
            .map_err(|_| unsupported(name))?;
        writeln!(out, "Sheet: {sheet}").expect("string write");
        sheet_rows(&range, &formulas, &mut out, name)?;
    }
    Ok(out)
}

/// Inclusive `((first_row, first_col), (last_row, last_col))` covering both
/// cached values and formulas, or `None` for an empty sheet.
fn sheet_bounds(
    range: &calamine::Range<calamine::Data>,
    formulas: &calamine::Range<String>,
) -> Option<((u32, u32), (u32, u32))> {
    let starts = [range.start(), formulas.start()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let ends = [range.end(), formulas.end()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if starts.is_empty() {
        return None;
    }
    let first = (
        starts.iter().map(|p| p.0).min().unwrap(),
        starts.iter().map(|p| p.1).min().unwrap(),
    );
    let last = (
        ends.iter().map(|p| p.0).max().unwrap(),
        ends.iter().map(|p| p.1).max().unwrap(),
    );
    Some((first, last))
}

/// A cell's cached value, else its formula marked as uncomputed, else empty.
fn sheet_cell(
    range: &calamine::Range<calamine::Data>,
    formulas: &calamine::Range<String>,
    position: (u32, u32),
) -> String {
    match range.get_value(position) {
        Some(cell) if !matches!(cell, calamine::Data::Empty) => cell_value(cell),
        _ => formulas
            .get_value(position)
            .filter(|s| !s.is_empty())
            .map(|formula| format!("={formula} (no cached result)"))
            .unwrap_or_default(),
    }
}

/// Appends one JSON array per row of the sheet's used area.
fn sheet_rows(
    range: &calamine::Range<calamine::Data>,
    formulas: &calamine::Range<String>,
    out: &mut String,
    name: &str,
) -> Result<(), Error> {
    let Some(((first_row, first_col), (last_row, last_col))) = sheet_bounds(range, formulas) else {
        return Ok(());
    };
    if u64::from(last_row - first_row + 1) * u64::from(last_col - first_col + 1) > 1_000_000 {
        return Err(Error::Large(name.into()));
    }
    for row in first_row..=last_row {
        let cells = (first_col..=last_col)
            .map(|col| sheet_cell(range, formulas, (row, col)))
            .collect::<Vec<_>>();
        out.push_str(&serde_json::to_string(&cells).expect("cell strings serialize"));
        out.push('\n');
        if out.len() as u64 > MAX_BYTES {
            return Err(Error::Large(name.into()));
        }
    }
    Ok(())
}

/// Internal relationship targets of `ppt/presentation.xml`, keyed by id.
fn slide_targets<'a>(relationships: &'a roxmltree::Document<'a>) -> HashMap<&'a str, &'a str> {
    relationships
        .descendants()
        .filter_map(|node| {
            if node.attribute("TargetMode") == Some("External") {
                return None;
            }
            Some((node.attribute("Id")?, node.attribute("Target")?))
        })
        .collect()
}

/// Each slide's text, in presentation order, under a `Slide <n>` header.
fn presentation(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<String, Error> {
    let presentation = xml(archive, "ppt/presentation.xml", name)?;
    let relationships = xml(archive, "ppt/_rels/presentation.xml.rels", name)?;
    let doc = roxmltree::Document::parse(&presentation).map_err(|_| unsupported(name))?;
    let rels = roxmltree::Document::parse(&relationships).map_err(|_| unsupported(name))?;
    let targets = slide_targets(&rels);
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
        out.push_str(&office_text(&xml(archive, &entry, name)?, name)?);
        if out.len() as u64 > MAX_BYTES {
            return Err(Error::Large(name.into()));
        }
    }
    Ok(out)
}

/// A rendered SVG side in whole pixels, at least one.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "scale caps the longest side at 1568 pixels and the side is at least 1"
)]
fn pixels(side: f32, scale: f32) -> u32 {
    (side * scale).ceil().max(1.0) as u32
}

pub(crate) fn svg(bytes: &[u8], name: &str) -> Result<Vec<u8>, Error> {
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
    let mut pixels =
        resvg::tiny_skia::Pixmap::new(pixels(size.width(), scale), pixels(size.height(), scale))
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
    use crate::process::{Limits, RunError, run_bounded};
    use std::{
        os::unix::fs::DirBuilderExt,
        process::{Command, Stdio},
        time::Duration,
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
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&temp.0)
        .map_err(|_| Error::Read(name.into()))?;
    let input = temp.0.join(format!("input.{extension}"));
    std::fs::write(&input, bytes).map_err(|_| Error::Read(name.into()))?;
    let output = if extension == "doc" { "docx" } else { "pptx" };
    let profile =
        url::Url::from_directory_path(temp.0.join("profile")).map_err(|()| unsupported(name))?;
    [
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
            .stdout(Stdio::null());
        match run_bounded(
            &mut command,
            None,
            Duration::from_secs(10),
            MAX_BYTES,
            None,
            Some(Limits::Office {
                max_file_bytes: MAX_BYTES,
            }),
        ) {
            Err(RunError::Spawn(_)) => None,
            result => Some(result),
        }
    })
    .ok_or_else(|| Error::OfficeUnavailable(name.into()))?
    .map_err(|_| unsupported(name))?;
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
        let input = r"<document><body><p><r><t>旅行 &amp; budget</t><t> €250</t></r></p><tbl><tr><tc><p><t>City</t></p></tc><tc><p><t>Fare</t></p></tc></tr><tr><tc><p><t>Seattle</t></p></tc><tc><p><t>42</t></p></tc></tr></tbl></body></document>";
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

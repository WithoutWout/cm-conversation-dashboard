//! A minimal `.xlsx` writer: one sheet, a bold frozen header row, column
//! widths, an autofilter, and an optional green highlight per row.
//!
//! Hand-written over the `zip` crate the updater already brings in, rather than
//! a spreadsheet crate: the whole format needed here is six small XML parts,
//! and every one of them is fixed text around the cells. Strings are written
//! inline (`t="inlineStr"`) so there is no shared-string table to keep in step.
//!
//! What has to be right, and is tested below: every cell is XML-escaped, the
//! characters XML 1.0 forbids are dropped rather than written (Excel refuses
//! the whole file over one stray control character from a chat log), a cell
//! is cut to Excel's 32,767-character limit, and the sheet name obeys Excel's
//! rules.

use serde::Deserialize;
use std::io::Write;

/// One cell. A number stays a number so the column sorts and sums as one.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum XlsxCell {
    Num(f64),
    Text(String),
    Null(()),
}

#[derive(Deserialize, Debug, Clone)]
pub struct XlsxRow {
    pub cells: Vec<XlsxCell>,
    /// Drawn with a light green fill — "this one is done".
    #[serde(default)]
    pub highlight: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct XlsxSheet {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<XlsxRow>,
    /// Column widths in characters; missing ones default to 14.
    #[serde(default)]
    pub widths: Vec<f64>,
}

/// Hard limits, checked before anything is written. The renderer never comes
/// near them; they exist so a malformed call cannot build a gigabyte in memory.
pub const MAX_ROWS: usize = 200_000;
pub const MAX_COLS: usize = 64;
const MAX_CELL_CHARS: usize = 32_767;

/// Excel's rules for a sheet name: at most 31 characters, none of `[]:*?/\`.
fn sheet_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | ':' | '*' | '?' | '/' | '\\'))
        .take(31)
        .collect();
    let trimmed = cleaned.trim().trim_matches('\'').to_string();
    if trimmed.is_empty() {
        "Sheet1".to_string()
    } else {
        trimmed
    }
}

/// XML-escape, dropping the characters XML 1.0 cannot carry at all.
fn xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (n, c) in s.chars().enumerate() {
        if n >= MAX_CELL_CHARS {
            break;
        }
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 => {}
            '\u{FFFE}' | '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// `0` → `A`, `26` → `AA`.
fn col_name(mut i: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push((b'A' + (i % 26) as u8) as char);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s.iter().rev().collect()
}

fn cell_xml(r: usize, c: usize, cell: &XlsxCell, style: u32) -> String {
    let at = format!("{}{}", col_name(c), r);
    let s = if style > 0 { format!(" s=\"{style}\"") } else { String::new() };
    match cell {
        XlsxCell::Num(v) if v.is_finite() => format!("<c r=\"{at}\"{s}><v>{v}</v></c>"),
        XlsxCell::Num(_) | XlsxCell::Null(()) => {
            if style > 0 {
                format!("<c r=\"{at}\"{s}/>")
            } else {
                String::new()
            }
        }
        XlsxCell::Text(t) => format!(
            "<c r=\"{at}\" t=\"inlineStr\"{s}><is><t xml:space=\"preserve\">{}</t></is></c>",
            xml_text(t)
        ),
    }
}

fn sheet_xml(sheet: &XlsxSheet) -> String {
    let ncols = sheet.headers.len().max(1);
    let mut x = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheetViews><sheetView workbookViewId=\"0\">\
         <pane ySplit=\"1\" topLeftCell=\"A2\" activePane=\"bottomLeft\" state=\"frozen\"/>\
         </sheetView></sheetViews><cols>",
    );
    for c in 0..ncols {
        let w = sheet.widths.get(c).copied().filter(|w| *w > 0.0 && *w < 256.0).unwrap_or(14.0);
        x.push_str(&format!(
            "<col min=\"{n}\" max=\"{n}\" width=\"{w}\" customWidth=\"1\"/>",
            n = c + 1
        ));
    }
    x.push_str("</cols><sheetData><row r=\"1\">");
    for (c, h) in sheet.headers.iter().enumerate() {
        x.push_str(&cell_xml(1, c, &XlsxCell::Text(h.clone()), 1));
    }
    x.push_str("</row>");
    for (i, row) in sheet.rows.iter().enumerate() {
        let r = i + 2;
        let style = if row.highlight { 2 } else { 3 };
        x.push_str(&format!("<row r=\"{r}\">"));
        for (c, cell) in row.cells.iter().take(ncols).enumerate() {
            x.push_str(&cell_xml(r, c, cell, style));
        }
        x.push_str("</row>");
    }
    let last = format!("{}{}", col_name(ncols - 1), sheet.rows.len() + 1);
    x.push_str(&format!(
        "</sheetData><autoFilter ref=\"A1:{last}\"/></worksheet>"
    ));
    x
}

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
</Types>";

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
</Relationships>";

const WORKBOOK_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
</Relationships>";

/// Style 0 default · 1 bold header on grey · 2 highlighted row · 3 plain row,
/// top-aligned and wrapping, so a long question does not push its row's
/// neighbours out of line.
const STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<fonts count=\"2\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font>\
<font><b/><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
<fills count=\"4\"><fill><patternFill patternType=\"none\"/></fill>\
<fill><patternFill patternType=\"gray125\"/></fill>\
<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFE8EAEE\"/><bgColor indexed=\"64\"/></patternFill></fill>\
<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFC6EFCE\"/><bgColor indexed=\"64\"/></patternFill></fill></fills>\
<borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
<cellXfs count=\"4\">\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
<xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"0\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\"/>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"3\" borderId=\"0\" xfId=\"0\" applyFill=\"1\" applyAlignment=\"1\"><alignment vertical=\"top\" wrapText=\"1\"/></xf>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyAlignment=\"1\"><alignment vertical=\"top\" wrapText=\"1\"/></xf>\
</cellXfs>\
<cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles>\
</styleSheet>";

/// The workbook as bytes, or why it could not be built.
pub fn build(sheet: &XlsxSheet) -> Result<Vec<u8>, String> {
    if sheet.headers.is_empty() {
        return Err("A sheet needs at least one column.".into());
    }
    if sheet.headers.len() > MAX_COLS {
        return Err(format!("At most {MAX_COLS} columns."));
    }
    if sheet.rows.len() > MAX_ROWS {
        return Err(format!("At most {MAX_ROWS} rows."));
    }
    let workbook = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheets><sheet name=\"{}\" sheetId=\"1\" r:id=\"rId1\"/></sheets>\
         <definedNames><definedName name=\"_xlnm._FilterDatabase\" localSheetId=\"0\" hidden=\"1\">'{}'!$A$1:${}${}</definedName></definedNames>\
         </workbook>",
        xml_text(&sheet_name(&sheet.name)),
        xml_text(&sheet_name(&sheet.name)),
        col_name(sheet.headers.len() - 1),
        sheet.rows.len() + 1
    );
    let mut buf = Vec::new();
    {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let parts: [(&str, String); 6] = [
            ("[Content_Types].xml", CONTENT_TYPES.to_string()),
            ("_rels/.rels", ROOT_RELS.to_string()),
            ("xl/workbook.xml", workbook),
            ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS.to_string()),
            ("xl/styles.xml", STYLES.to_string()),
            ("xl/worksheets/sheet1.xml", sheet_xml(sheet)),
        ];
        for (name, body) in parts.iter() {
            w.start_file(*name, opts).map_err(|e| e.to_string())?;
            w.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        }
        w.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn part(bytes: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("a zip");
        let mut f = z.by_name(name).expect(name);
        let mut s = String::new();
        f.read_to_string(&mut s).expect("utf-8");
        s
    }

    fn sheet(rows: Vec<XlsxRow>) -> XlsxSheet {
        XlsxSheet {
            name: "GAP".into(),
            headers: vec!["Question".into(), "Recognition %".into()],
            rows,
            widths: vec![60.0, 12.0],
        }
    }

    #[test]
    fn a_workbook_has_every_part_excel_needs() {
        let bytes = build(&sheet(vec![])).expect("build");
        for name in [
            "[Content_Types].xml",
            "_rels/.rels",
            "xl/workbook.xml",
            "xl/_rels/workbook.xml.rels",
            "xl/styles.xml",
            "xl/worksheets/sheet1.xml",
        ] {
            part(&bytes, name);
        }
        let ws = part(&bytes, "xl/worksheets/sheet1.xml");
        assert!(ws.contains("state=\"frozen\""), "the header row is frozen");
        assert!(ws.contains("<autoFilter ref=\"A1:B1\"/>"));
    }

    #[test]
    fn text_is_escaped_and_forbidden_characters_are_dropped() {
        let bytes = build(&sheet(vec![XlsxRow {
            cells: vec![
                XlsxCell::Text("a <b> & \"c\" \u{0001}\u{0008}ok\u{FFFF}".into()),
                XlsxCell::Num(42.5),
            ],
            highlight: true,
        }]))
        .expect("build");
        let ws = part(&bytes, "xl/worksheets/sheet1.xml");
        assert!(ws.contains("a &lt;b&gt; &amp; &quot;c&quot; ok</t>"), "{ws}");
        assert!(ws.contains("<c r=\"B2\" s=\"2\"><v>42.5</v></c>"), "a number stays a number, highlighted");
        assert!(!ws.contains('\u{0001}'));
    }

    #[test]
    fn a_cell_is_cut_to_excels_limit_and_names_follow_its_rules() {
        let long = "x".repeat(40_000);
        let bytes = build(&sheet(vec![XlsxRow {
            cells: vec![XlsxCell::Text(long), XlsxCell::Null(())],
            highlight: false,
        }]))
        .expect("build");
        let ws = part(&bytes, "xl/worksheets/sheet1.xml");
        assert!(ws.contains(&"x".repeat(MAX_CELL_CHARS)));
        assert!(!ws.contains(&"x".repeat(MAX_CELL_CHARS + 1)));
        assert_eq!(sheet_name("GAP [2026/09]: *all*?"), "GAP 202609 all");
        assert_eq!(sheet_name("   "), "Sheet1");
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
    }

    #[test]
    fn a_cell_parses_from_the_renderers_json() {
        let rows: Vec<XlsxRow> =
            serde_json::from_str(r#"[{"cells":["q", 12.5, null], "highlight": true}, {"cells":[]}]"#)
                .expect("json");
        assert!(matches!(rows[0].cells[0], XlsxCell::Text(_)));
        assert!(matches!(rows[0].cells[1], XlsxCell::Num(_)));
        assert!(matches!(rows[0].cells[2], XlsxCell::Null(())));
        assert!(!rows[1].highlight);
    }
}


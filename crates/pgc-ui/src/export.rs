//! Result export to CSV, JSON and XML (D-021, D-042).
//!
//! [`Exporter`] writes a header, then rows in batches, then the closing part. Values go
//! through the crates' APIs (`csv`, `serde_json`, `quick-xml`), so escaping is never
//! done by hand. [`spawn`] runs an exporter on its own thread, so file writing never
//! blocks the UI; the UI hands it batches and waits for each to be written before it
//! fetches the next one (backpressure, as for the grid).

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use base64::Engine;
use pgc_core::settings::{CsvSeparator, ExportFormat};
use pgc_db::query::{Column, Value};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};

/// Options of one export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub separator: CsvSeparator,
    pub bom: bool,
}

/// How a JSON value of a column is written (D-042).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonKind {
    Number,
    Bool,
    Json,
    Text,
}

fn json_kind(type_oid: u32) -> JsonKind {
    match type_oid {
        // int8, int2, int4, oid, float4, float8, numeric
        20 | 21 | 23 | 26 | 700 | 701 | 1700 => JsonKind::Number,
        16 => JsonKind::Bool,
        // json, jsonb
        114 | 3802 => JsonKind::Json,
        _ => JsonKind::Text,
    }
}

/// `s` is a number in JSON syntax (so `NaN`, `Infinity`, `1.` or `+1` are not).
fn is_json_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    if b.first() == Some(&b'-') {
        i += 1;
    }
    match b.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => {
            digits(&mut i);
        }
        _ => return false,
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len()
}

/// XML 1.0 forbids this character even when escaped.
fn xml_forbidden(c: char) -> bool {
    (c < ' ' && !matches!(c, '\t' | '\n' | '\r')) || matches!(c, '\u{FFFE}' | '\u{FFFF}')
}

/// Column names made unique for JSON keys: `a`, `a_2`, `a_3`, ... (D-021).
fn unique_names(columns: &[Column]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(columns.len());
    for c in columns {
        let mut name = c.name.clone();
        let mut n = 1;
        while out.contains(&name) {
            n += 1;
            name = format!("{}_{n}", c.name);
        }
        out.push(name);
    }
    out
}

enum Inner<W: Write> {
    Csv(Box<csv::Writer<W>>),
    Json {
        out: W,
        keys: Vec<String>,
        kinds: Vec<JsonKind>,
    },
    Xml(quick_xml::Writer<W>),
}

/// Writes one result set in a format, row batch by row batch.
pub struct Exporter<W: Write> {
    inner: Inner<W>,
    rows: u64,
}

fn xml_err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::other(e)
}

impl<W: Write> Exporter<W> {
    /// Starts the file: BOM, header row / opening part.
    pub fn new(mut out: W, options: ExportOptions, columns: &[Column]) -> io::Result<Self> {
        let inner = match options.format {
            ExportFormat::Csv => {
                if options.bom {
                    out.write_all("\u{FEFF}".as_bytes())?;
                }
                let mut w = csv::WriterBuilder::new()
                    .delimiter(options.separator.as_byte())
                    .terminator(csv::Terminator::CRLF)
                    .from_writer(out);
                w.write_record(columns.iter().map(|c| c.name.as_str()))?;
                Inner::Csv(Box::new(w))
            }
            ExportFormat::Json => {
                out.write_all(b"[")?;
                Inner::Json {
                    out,
                    keys: unique_names(columns),
                    kinds: columns.iter().map(|c| json_kind(c.type_oid)).collect(),
                }
            }
            ExportFormat::Xml => {
                let mut w = quick_xml::Writer::new(out);
                w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
                    .map_err(xml_err)?;
                w.get_mut().write_all(b"\n")?;
                w.write_event(Event::Start(BytesStart::new("result")))
                    .map_err(xml_err)?;
                w.get_mut().write_all(b"\n  ")?;
                w.write_event(Event::Start(BytesStart::new("columns")))
                    .map_err(xml_err)?;
                for c in columns {
                    let mut el = BytesStart::new("column");
                    if c.name.chars().any(xml_forbidden) {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&c.name);
                        el.push_attribute(("name", encoded.as_str()));
                        el.push_attribute(("encoding", "base64"));
                    } else {
                        el.push_attribute(("name", c.name.as_str()));
                    }
                    w.write_event(Event::Empty(el)).map_err(xml_err)?;
                }
                w.write_event(Event::End(BytesEnd::new("columns")))
                    .map_err(xml_err)?;
                Inner::Xml(w)
            }
        };
        Ok(Self { inner, rows: 0 })
    }

    /// Rows written so far.
    pub fn rows(&self) -> u64 {
        self.rows
    }

    pub fn write_rows(&mut self, rows: &[Vec<Value>]) -> io::Result<()> {
        for row in rows {
            match &mut self.inner {
                Inner::Csv(w) => {
                    // NULL as an empty field (D-021)
                    w.write_record(row.iter().map(|v| v.as_deref().unwrap_or("")))?;
                }
                Inner::Json { out, keys, kinds } => {
                    out.write_all(if self.rows == 0 { b"\n  {" } else { b",\n  {" })?;
                    for (i, v) in row.iter().enumerate() {
                        if i > 0 {
                            out.write_all(b", ")?;
                        }
                        let key = keys.get(i).map_or("", String::as_str);
                        serde_json::to_writer(&mut *out, key)?;
                        out.write_all(b": ")?;
                        let kind = kinds.get(i).copied().unwrap_or(JsonKind::Text);
                        write_json_value(out, v.as_deref(), kind)?;
                    }
                    out.write_all(b"}")?;
                }
                Inner::Xml(w) => {
                    w.get_mut().write_all(b"\n  ")?;
                    w.write_event(Event::Start(BytesStart::new("row")))
                        .map_err(xml_err)?;
                    for v in row {
                        write_xml_value(w, v.as_deref())?;
                    }
                    w.write_event(Event::End(BytesEnd::new("row")))
                        .map_err(xml_err)?;
                }
            }
            self.rows += 1;
        }
        Ok(())
    }

    /// Writes the closing part and flushes; returns the output.
    pub fn finish(self) -> io::Result<W> {
        let mut out = match self.inner {
            Inner::Csv(w) => w.into_inner().map_err(|e| e.into_error())?,
            Inner::Json { mut out, .. } => {
                out.write_all(if self.rows == 0 { b"]\n" } else { b"\n]\n" })?;
                out
            }
            Inner::Xml(mut w) => {
                w.get_mut().write_all(b"\n")?;
                w.write_event(Event::End(BytesEnd::new("result")))
                    .map_err(xml_err)?;
                let mut out = w.into_inner();
                out.write_all(b"\n")?;
                out
            }
        };
        out.flush()?;
        Ok(out)
    }
}

fn write_json_value<W: Write>(out: &mut W, v: Option<&str>, kind: JsonKind) -> io::Result<()> {
    let Some(s) = v else {
        return out.write_all(b"null");
    };
    match kind {
        // exactly as the server sent it: no float conversion (D-042)
        JsonKind::Number if is_json_number(s) => out.write_all(s.as_bytes()),
        JsonKind::Bool if s == "t" => out.write_all(b"true"),
        JsonKind::Bool if s == "f" => out.write_all(b"false"),
        JsonKind::Json if serde_json::from_str::<serde::de::IgnoredAny>(s).is_ok() => {
            out.write_all(s.as_bytes())
        }
        _ => Ok(serde_json::to_writer(out, s)?),
    }
}

fn write_xml_value<W: Write>(w: &mut quick_xml::Writer<W>, v: Option<&str>) -> io::Result<()> {
    let mut el = BytesStart::new("value");
    let Some(s) = v else {
        el.push_attribute(("null", "true"));
        return w.write_event(Event::Empty(el)).map_err(xml_err);
    };
    if s.chars().any(xml_forbidden) {
        el.push_attribute(("encoding", "base64"));
        let encoded = base64::engine::general_purpose::STANDARD.encode(s);
        w.write_event(Event::Start(el)).map_err(xml_err)?;
        w.write_event(Event::Text(BytesText::new(&encoded)))
            .map_err(xml_err)?;
    } else {
        w.write_event(Event::Start(el)).map_err(xml_err)?;
        w.write_event(Event::Text(BytesText::new(s)))
            .map_err(xml_err)?;
    }
    w.write_event(Event::End(BytesEnd::new("value")))
        .map_err(xml_err)
}

// ---- writer thread -------------------------------------------------------------------

/// UI → writer thread.
pub enum ToWriter {
    Rows(Vec<Vec<Value>>),
    /// All rows sent: close the file.
    Finish,
    /// Stop and delete the file.
    Abort,
}

/// Writer thread → UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportEvent {
    /// A batch is written; `rows` in total so far.
    Written {
        id: u64,
        rows: u64,
    },
    Done {
        id: u64,
        rows: u64,
    },
    /// Writing failed; the file is deleted.
    Failed {
        id: u64,
        error: String,
    },
    /// Aborted; the file is deleted.
    Aborted {
        id: u64,
    },
}

pub type ExportSink = Arc<dyn Fn(ExportEvent) + Send + Sync>;

/// Starts export `id` to `path` on its own thread.
pub fn spawn(
    id: u64,
    path: PathBuf,
    options: ExportOptions,
    columns: Vec<Column>,
    sink: ExportSink,
) -> mpsc::Sender<ToWriter> {
    let (tx, rx) = mpsc::channel::<ToWriter>();
    std::thread::spawn(move || {
        let fail = |error: String| {
            let _ = std::fs::remove_file(&path);
            sink(ExportEvent::Failed { id, error });
        };
        let mut exporter = match File::create(&path)
            .and_then(|f| Exporter::new(BufWriter::new(f), options, &columns))
        {
            Ok(e) => e,
            Err(e) => {
                fail(format!("Cannot write {}:\n{e}", path.display()));
                return;
            }
        };
        loop {
            match rx.recv() {
                Ok(ToWriter::Rows(rows)) => {
                    if let Err(e) = exporter.write_rows(&rows) {
                        drop(exporter);
                        fail(format!("Cannot write {}:\n{e}", path.display()));
                        return;
                    }
                    sink(ExportEvent::Written {
                        id,
                        rows: exporter.rows(),
                    });
                }
                Ok(ToWriter::Finish) => {
                    let rows = exporter.rows();
                    match exporter.finish() {
                        Ok(_) => sink(ExportEvent::Done { id, rows }),
                        Err(e) => fail(format!("Cannot write {}:\n{e}", path.display())),
                    }
                    return;
                }
                // Abort, or the UI side is gone (tab closed, quit)
                Ok(ToWriter::Abort) | Err(_) => {
                    drop(exporter);
                    let _ = std::fs::remove_file(&path);
                    sink(ExportEvent::Aborted { id });
                    return;
                }
            }
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, type_oid: u32) -> Column {
        Column {
            name: name.to_owned(),
            type_oid,
        }
    }

    fn v(s: &str) -> Value {
        Some(s.into())
    }

    fn export(options: ExportOptions, columns: &[Column], rows: &[Vec<Value>]) -> String {
        let mut e = Exporter::new(Vec::new(), options, columns).unwrap();
        // two batches, as from the stream
        let (a, b) = rows.split_at(rows.len() / 2);
        e.write_rows(a).unwrap();
        e.write_rows(b).unwrap();
        String::from_utf8(e.finish().unwrap()).unwrap()
    }

    fn opts(format: ExportFormat) -> ExportOptions {
        ExportOptions {
            format,
            separator: CsvSeparator::Comma,
            bom: false,
        }
    }

    #[test]
    fn json_numbers() {
        for ok in [
            "0",
            "-1",
            "12.50",
            "1e10",
            "-0.5E-3",
            "123456789012345678901234567890",
        ] {
            assert!(is_json_number(ok), "{ok}");
        }
        for bad in [
            "",
            "-",
            "01",
            "1.",
            ".5",
            "+1",
            "NaN",
            "Infinity",
            "-Infinity",
            "1e",
            "1 ",
        ] {
            assert!(!is_json_number(bad), "{bad}");
        }
    }

    #[test]
    fn csv_quotes_and_nulls() {
        let cols = [col("id", 23), col("text; \"quoted\"", 25)];
        let rows = [
            vec![v("1"), v("a,b")],
            vec![v("2"), None],
            vec![v("3"), v("line\nbreak \"q\"")],
            vec![v("4"), v("č;š")],
        ];
        let out = export(opts(ExportFormat::Csv), &cols, &rows);
        assert_eq!(
            out,
            "id,\"text; \"\"quoted\"\"\"\r\n1,\"a,b\"\r\n2,\r\n3,\"line\nbreak \"\"q\"\"\"\r\n4,č;š\r\n"
        );
        let semi = ExportOptions {
            separator: CsvSeparator::Semicolon,
            bom: true,
            ..opts(ExportFormat::Csv)
        };
        let out = export(semi, &cols, &rows);
        assert!(out.starts_with("\u{FEFF}id;\"text; "), "{out}");
        assert!(out.contains("\r\n1;a,b\r\n"), "{out}");
        assert!(out.ends_with("4;\"č;š\"\r\n"), "{out}");
        let tab = ExportOptions {
            separator: CsvSeparator::Tab,
            ..opts(ExportFormat::Csv)
        };
        assert!(export(tab, &cols, &rows).starts_with("id\t"));
    }

    #[test]
    fn json_is_typed() {
        let cols = [
            col("id", 23),
            col("price", 1700),
            col("f", 701),
            col("ok", 16),
            col("doc", 3802),
            col("id", 25),
            col("note", 25),
        ];
        let rows = [
            vec![
                v("1"),
                v("12345678901234567890.10"),
                v("NaN"),
                v("t"),
                v(r#"{"a": [1, 2]}"#),
                v("Čaša \"x\"\n"),
                None,
            ],
            vec![
                v("2"),
                v("-0.5"),
                v("1e+300"),
                v("f"),
                v("null"),
                v(""),
                v("\u{1}"),
            ],
        ];
        let out = export(opts(ExportFormat::Json), &cols, &rows);
        assert_eq!(
            out,
            "[\n  {\"id\": 1, \"price\": 12345678901234567890.10, \"f\": \"NaN\", \"ok\": true, \
             \"doc\": {\"a\": [1, 2]}, \"id_2\": \"Čaša \\\"x\\\"\\n\", \"note\": null},\n  \
             {\"id\": 2, \"price\": -0.5, \"f\": 1e+300, \"ok\": false, \"doc\": null, \
             \"id_2\": \"\", \"note\": \"\\u0001\"}\n]\n"
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed[0]["id_2"], "Čaša \"x\"\n");
        assert_eq!(export(opts(ExportFormat::Json), &cols, &[]), "[]\n");
    }

    #[test]
    fn xml_layout_nulls_and_base64() {
        let cols = [col("id", 23), col("a<b & \"c\"", 25)];
        let rows = [
            vec![v("1"), v("Čaša <x> & y")],
            vec![v("2"), None],
            vec![v("3"), v("bell\u{7}")],
            vec![v("4"), v("")],
        ];
        let out = export(opts(ExportFormat::Xml), &cols, &rows);
        let b64 = base64::engine::general_purpose::STANDARD.encode("bell\u{7}");
        assert_eq!(
            out,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<result>\n  \
                 <columns><column name=\"id\"/><column name=\"a&lt;b &amp; &quot;c&quot;\"/></columns>\n  \
                 <row><value>1</value><value>Čaša &lt;x&gt; &amp; y</value></row>\n  \
                 <row><value>2</value><value null=\"true\"/></row>\n  \
                 <row><value>3</value><value encoding=\"base64\">{b64}</value></row>\n  \
                 <row><value>4</value><value></value></row>\n</result>\n"
            )
        );
    }

    #[test]
    fn writer_thread_writes_and_deletes_on_abort() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let sink: ExportSink = Arc::new(move |e| {
            let _ = tx.send(e);
        });
        let cols = vec![col("n", 23)];
        // finished export
        let path = dir.path().join("a.csv");
        let w = spawn(
            1,
            path.clone(),
            opts(ExportFormat::Csv),
            cols.clone(),
            sink.clone(),
        );
        w.send(ToWriter::Rows(vec![vec![v("1")], vec![v("2")]]))
            .unwrap();
        assert_eq!(rx.recv().unwrap(), ExportEvent::Written { id: 1, rows: 2 });
        w.send(ToWriter::Finish).unwrap();
        assert_eq!(rx.recv().unwrap(), ExportEvent::Done { id: 1, rows: 2 });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "n\r\n1\r\n2\r\n");
        // aborted export: the file is gone
        let path = dir.path().join("b.csv");
        let w = spawn(
            2,
            path.clone(),
            opts(ExportFormat::Csv),
            cols.clone(),
            sink.clone(),
        );
        w.send(ToWriter::Rows(vec![vec![v("1")]])).unwrap();
        assert_eq!(rx.recv().unwrap(), ExportEvent::Written { id: 2, rows: 1 });
        w.send(ToWriter::Abort).unwrap();
        assert_eq!(rx.recv().unwrap(), ExportEvent::Aborted { id: 2 });
        assert!(!path.exists());
        // cannot create the file
        let path = dir.path().join("missing").join("c.csv");
        let _w = spawn(3, path, opts(ExportFormat::Csv), cols, sink);
        assert!(matches!(
            rx.recv().unwrap(),
            ExportEvent::Failed { id: 3, .. }
        ));
    }
}

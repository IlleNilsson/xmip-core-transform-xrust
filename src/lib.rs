#![forbid(unsafe_code)]

//! The xrust XSLT engine — a technology of `xmip-core-transform`, the first
//! engine the Stylesheet unit chooses among (the owner, 2026-10-22: *There
//! are many XSLT transform engines, versions, let the end user choose*).
//!
//! `xrust` (Apache-2.0) implements XSLT 1.0 over the XDM 3.1 data model;
//! a step asking for a later version through it is refused when it is
//! opened. Its trees are built on `Rc`, so a compiled stylesheet lives on
//! the thread that uses it: it is compiled when its step is opened, which
//! refuses a stylesheet that does not compile, and once more on each thread
//! that applies it, never per Message.
//!
//! Offline (ADR-0045, amendment 2026-10-22): `xrust` reads nothing itself
//! and asks its caller for every document an `xsl:include`, `xsl:import`
//! or `document()` names. This engine answers from the stylesheet's own
//! folder and below alone, and refuses every other address — `http:`,
//! `https:` and anything not a file there — in words, before anything is
//! read.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use transform::stylesheet::{CompiledStylesheet, StylesheetEngine};
use url::Url;
use xrust::item::{Item, Node, SequenceTrait};
use xrust::parser::ParseError;
use xrust::transform::context::{Context, StaticContextBuilder};
use xrust::trees::smite::RNode;
use xrust::xdmerror::{Error, ErrorKind};

/// The engine, as a step's `engine` names it: `xrust`.
pub struct Xrust;

impl StylesheetEngine for Xrust {
    fn name(&self) -> &'static str {
        "xrust"
    }

    fn versions(&self) -> &'static [&'static str] {
        &["1.0"]
    }

    fn compile(
        &self,
        stylesheet: &str,
        place: &Path,
    ) -> Result<Box<dyn CompiledStylesheet>, String> {
        let folder = place
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
            .canonicalize()
            .map_err(|failed| format!("the stylesheet's folder cannot be read: {failed}"))?;
        let compiled = Compiled {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            stylesheet: stylesheet.to_string(),
            folder,
        };
        compiled.context()?;
        Ok(Box::new(compiled))
    }
}

/// The next compiled stylesheet's number, by which each thread keeps its
/// own compilation of it.
static NEXT: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Each stylesheet this thread applied, compiled once on it.
    static COMPILED: RefCell<HashMap<u64, Context<RNode>>> = RefCell::new(HashMap::new());
}

/// A stylesheet, compiled once on each thread that applies it.
struct Compiled {
    id: u64,
    stylesheet: String,
    folder: PathBuf,
}

impl Compiled {
    fn context(&self) -> Result<Context<RNode>, String> {
        let folder = self.folder.clone();
        let base = Url::from_directory_path(&self.folder).ok();
        let style = document(&self.stylesheet).map_err(|failed| failed.to_string())?;
        xrust::xslt::from_document(style, base, document, move |url| local(&folder, url))
            .map_err(|failed| failed.to_string())
    }
}

impl CompiledStylesheet for Compiled {
    fn apply(&self, input: &mut dyn Read, output: &mut dyn Write) -> Result<(), String> {
        let mut source = String::new();
        input
            .read_to_string(&mut source)
            .map_err(|failed| format!("the document is not UTF-8 text: {failed}"))?;
        let source = document(&source).map_err(|failed| format!("the document: {failed}"))?;
        let written = COMPILED.with(|compiled| -> Result<String, String> {
            let mut compiled = compiled.borrow_mut();
            let context = match compiled.entry(self.id) {
                Entry::Occupied(found) => found.into_mut(),
                Entry::Vacant(vacant) => vacant.insert(self.context()?),
            };
            context.context(vec![Item::Node(source)], 0);
            context.result_document(RNode::new_document());
            let folder = self.folder.clone();
            let mut statics = StaticContextBuilder::new()
                .message(|_| Ok(()))
                .parser(|text: &str| document(text))
                .fetcher(move |url: &Url| local(&folder, url))
                .build();
            let sequence = context
                .evaluate(&mut statics)
                .map_err(|failed| failed.to_string())?;
            Ok(sequence.to_xml())
        })?;
        output
            .write_all(written.as_bytes())
            .and_then(|()| output.flush())
            .map_err(|failed| format!("the result could not be written: {failed}"))
    }
}

/// `text` parsed as an XML document.
fn document(text: &str) -> Result<RNode, Error> {
    let document = RNode::new_document();
    xrust::parser::xml::parse(
        document.clone(),
        text,
        Some(|_: &_| {
            Err(ParseError::MissingNameSpace {
                prefix: None,
                nsuri: None,
                message: Some("a prefix that names no namespace".to_string()),
            })
        }),
    )?;
    Ok(document)
}

/// The text a stylesheet names at `url`, read from `folder` or below it
/// alone; any other address is refused, never fetched.
fn local(folder: &Path, url: &Url) -> Result<String, Error> {
    let refused = |why: &str| Error::new(ErrorKind::Unknown, format!("'{url}' {why}"));
    if url.scheme() != "file" {
        return Err(refused(
            "is not read: Xmip fetches nothing over a network (ADR-0045); \
             put it beside the stylesheet",
        ));
    }
    let path = url
        .to_file_path()
        .map_err(|()| refused("names no file"))?
        .canonicalize()
        .map_err(|_| refused("names no file there is"))?;
    if !path.starts_with(folder) {
        return Err(refused("lies outside the stylesheet's folder"));
    }
    std::fs::read_to_string(&path).map_err(|failed| refused(&format!("cannot be read: {failed}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLESHEET: &str = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform'>\
<xsl:template match='/'><Invoice><xsl:apply-templates select='Order/Id'/></Invoice></xsl:template>\
<xsl:template match='Id'><Number><xsl:value-of select='.'/></Number></xsl:template>\
</xsl:stylesheet>";

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("xmip-xrust-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("a folder");
        folder
    }

    fn applied(compiled: &dyn CompiledStylesheet, document: &str) -> Result<String, String> {
        let mut written = Vec::new();
        compiled.apply(&mut document.as_bytes(), &mut written)?;
        Ok(String::from_utf8_lossy(&written).into_owned())
    }

    #[test]
    fn a_stylesheet_compiles_once_and_transforms_on_any_thread() {
        let place = folder("apply").join("invoice.xsl");
        let compiled = Xrust.compile(STYLESHEET, &place).expect("compiles");
        assert_eq!(
            applied(&*compiled, "<Order><Id>A-1</Id></Order>").expect("applies"),
            "<Invoice><Number>A-1</Number></Invoice>"
        );
        let compiled: std::sync::Arc<dyn CompiledStylesheet> = compiled.into();
        let elsewhere = std::sync::Arc::clone(&compiled);
        let other = std::thread::spawn(move || applied(&*elsewhere, "<Order><Id>B-2</Id></Order>"))
            .join()
            .expect("the other thread");
        assert_eq!(
            other.expect("applies"),
            "<Invoice><Number>B-2</Number></Invoice>"
        );
        assert!(applied(&*compiled, "<Order>").is_err());
    }

    #[test]
    fn a_remote_include_is_refused_without_a_network_attempt() {
        let place = folder("remote").join("remote.xsl");
        let remote = "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform'>\
<xsl:include href='https://example.test/common.xsl'/></xsl:stylesheet>";
        let refused = Xrust.compile(remote, &place).err().expect("refused");
        assert!(
            refused.contains("fetches nothing over a network"),
            "{refused}"
        );
        // A file that is there, beside the stylesheet's folder and not in it:
        // `file:///` names no file on Windows, which has no root without a
        // drive, and would be refused before the folder is asked.
        let inside = folder("inside").canonicalize().expect("canonical");
        let beside = folder("beside").join("common.xsl");
        std::fs::write(&beside, STYLESHEET).expect("written");
        let outside = Url::from_file_path(&beside).expect("a url");
        let refused = local(&inside, &outside).expect_err("refused").to_string();
        assert!(
            refused.contains("outside the stylesheet's folder"),
            "{refused}"
        );
    }
}

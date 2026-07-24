//! Inspect an EPUB without invoking a browser or renderer.

use std::env;
use std::process::ExitCode;

use rebook_epub::EpubPublication;
use rebook_publication::BookSource;
use serde::Serialize;

#[derive(Serialize)]
struct Inspection<'a> {
    id: String,
    metadata: &'a rebook_publication::Metadata,
    cover: Option<&'a rebook_publication::PublicationUrl>,
    manifest: &'a [rebook_publication::Link],
    reading_order: &'a [rebook_publication::SpineItem],
    table_of_contents: &'a [rebook_publication::TocEntry],
    diagnostics: &'a [rebook_epub::Diagnostic],
}

fn main() -> ExitCode {
    let mut arguments = env::args_os();
    let executable = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "rebook-inspect".into());
    let Some(path) = arguments.next() else {
        eprintln!("usage: {executable} <book.epub>");
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!("usage: {executable} <book.epub>");
        return ExitCode::from(2);
    }

    match inspect(path) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to inspect EPUB: {error}");
            ExitCode::FAILURE
        }
    }
}

fn inspect(path: std::ffi::OsString) -> Result<String, Box<dyn std::error::Error>> {
    let publication = EpubPublication::open_file(path)?;
    let inspection = Inspection {
        id: publication.book().id.to_string(),
        metadata: &publication.book().metadata,
        cover: publication.book().cover.as_ref(),
        manifest: publication.manifest(),
        reading_order: &publication.book().sections,
        table_of_contents: &publication.book().table_of_contents,
        diagnostics: publication.diagnostics(),
    };
    serde_json::to_string_pretty(&inspection).map_err(Into::into)
}

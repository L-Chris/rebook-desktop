//! Inspect an e-book without invoking a browser or renderer.

use std::env;
use std::process::ExitCode;

use rebook_formats::open_file;
use serde::Serialize;

#[derive(Serialize)]
struct Inspection<'a> {
    format: String,
    id: String,
    metadata: &'a rebook_publication::Metadata,
    cover: Option<&'a rebook_publication::PublicationUrl>,
    reading_order: &'a [rebook_publication::SpineItem],
    table_of_contents: &'a [rebook_publication::TocEntry],
    parsed_sections: Vec<SectionInspection>,
}

#[derive(Serialize)]
struct SectionInspection {
    index: usize,
    block_count: usize,
    anchor_count: usize,
}

fn main() -> ExitCode {
    let mut arguments = env::args_os();
    let executable = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "rebook-inspect".into());
    let Some(path) = arguments.next() else {
        eprintln!("usage: {executable} <book>");
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!("usage: {executable} <book>");
        return ExitCode::from(2);
    }

    match inspect(path) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to inspect e-book: {error}");
            ExitCode::FAILURE
        }
    }
}

fn inspect(path: std::ffi::OsString) -> Result<String, Box<dyn std::error::Error>> {
    let opened = open_file(std::path::PathBuf::from(path))?;
    let source = opened.source();
    let publication = source.book();
    let parsed_sections = (0..publication.sections.len())
        .map(|index| {
            let section = source.parse_section(index)?;
            Ok(SectionInspection {
                index,
                block_count: section.blocks.len(),
                anchor_count: section.anchors.len(),
            })
        })
        .collect::<Result<Vec<_>, rebook_publication::PublicationError>>()?;
    let inspection = Inspection {
        format: opened.format().label().to_owned(),
        id: publication.id.to_string(),
        metadata: &publication.metadata,
        cover: publication.cover.as_ref(),
        reading_order: &publication.sections,
        table_of_contents: &publication.table_of_contents,
        parsed_sections,
    };
    serde_json::to_string_pretty(&inspection).map_err(Into::into)
}

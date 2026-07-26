//! Native e-book reader: parser -> reading IR -> page layout -> display list -> Xilem/Vello.

mod app;
mod async_task;
mod fonts;
mod highlights;
mod library;
mod persistence;
mod plugins;
mod preferences;
mod reader;
mod shelf;
mod sync;
mod ui;

use std::env;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use app::{DesktopApp, root_view};
use library::LocalLibrary;
use lucide_icons::LUCIDE_FONT_BYTES;
use xilem::{EventLoop, WindowOptions, Xilem};

const INITIAL_WIDTH: u32 = 1200;
const INITIAL_HEIGHT: u32 = 800;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rebook-desktop failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let launch = parse_arguments()?;
    let reader_fonts = fonts::embedded_reader_fonts();

    let library =
        LocalLibrary::load_default().map_err(|error| io::Error::other(error.to_string()))?;
    let mut state = DesktopApp::new(library, Arc::clone(&reader_fonts));
    if let LaunchMode::Open(path) = launch {
        state.open_book(&path);
    }
    let window = WindowOptions::new("Rebook")
        .with_initial_inner_size(xilem::winit::dpi::LogicalSize::new(
            INITIAL_WIDTH,
            INITIAL_HEIGHT,
        ))
        .with_min_inner_size(xilem::winit::dpi::LogicalSize::new(720_u32, 520_u32));
    let mut application =
        Xilem::new_simple(state, root_view, window).with_font(LUCIDE_FONT_BYTES.to_vec());
    for font in reader_fonts.iter() {
        application = application.with_font(font.clone());
    }
    application.run_in(EventLoop::with_user_event())?;
    Ok(())
}

enum LaunchMode {
    Shelf,
    Open(PathBuf),
}

fn parse_arguments() -> Result<LaunchMode, Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let executable = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "rebook-desktop".into());
    let Some(first) = arguments.next() else {
        return Ok(LaunchMode::Shelf);
    };
    let launch = LaunchMode::Open(PathBuf::from(first));
    if arguments.next().is_some() {
        return Err(usage(&executable).into());
    }
    Ok(launch)
}

fn usage(executable: &str) -> String {
    format!("usage: {executable} [book]")
}

//! Phase 0 native window: EPUB -> Publication -> Stylo/Taffy/Parley -> Vello/wgpu.

use std::collections::HashSet;
use std::env;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use anyrender_vello::VelloWindowRenderer;
use blitz_shell::{BlitzApplication, BlitzShellProxy, WindowConfig, create_default_event_loop};
use rebook_epub::EpubPublication;
use rebook_renderer::{LayoutViewport, ReflowDocument};

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
    let mut arguments = env::args_os();
    let executable = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "rebook-desktop".into());
    let Some(first) = arguments.next() else {
        return Err(usage(&executable).into());
    };
    let (diagnose, path) = if first == "--diagnose" {
        (true, arguments.next().ok_or_else(|| usage(&executable))?)
    } else {
        (false, first)
    };
    if arguments.next().is_some() {
        return Err(usage(&executable).into());
    }

    let started = Instant::now();
    let publication = Arc::new(EpubPublication::open_file(path)?);
    let opened = Instant::now();
    let viewport = LayoutViewport::new(900, 700, 1.0)?;
    let mut document = ReflowDocument::layout(publication, 0, viewport)?;
    let laid_out = Instant::now();

    if diagnose {
        let metrics = document.metrics();
        let failures = document.resource_failures();
        let rgba = document.render_offscreen_rgba(viewport)?;
        let painted = Instant::now();
        let distinct_colors = rgba
            .chunks_exact(4)
            .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
            .collect::<HashSet<_>>()
            .len();
        let non_transparent_pixels = rgba.chunks_exact(4).filter(|pixel| pixel[3] != 0).count();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "backend": "Vello CPU offscreen",
                "open_ms": elapsed_ms(started, opened),
                "layout_ms": elapsed_ms(opened, laid_out),
                "paint_ms": elapsed_ms(laid_out, painted),
                "total_ms": elapsed_ms(started, painted),
                "process_peak_resident_kib": process_peak_resident_kib(),
                "layout": {
                    "content_width": metrics.content_width,
                    "content_height": metrics.content_height,
                    "node_count": metrics.node_count,
                    "pending_critical_resources": metrics.has_pending_critical_resources,
                },
                "render_target": {
                    "width": viewport.width,
                    "height": viewport.height,
                    "rgba_bytes": rgba.len(),
                    "non_transparent_pixels": non_transparent_pixels,
                    "distinct_colors": distinct_colors,
                },
                "resource_failures": failures.iter().map(|failure| failure.url.as_str()).collect::<Vec<_>>(),
            }))?
        );
        return Ok(());
    }

    let event_loop = create_default_event_loop();
    let (proxy, event_queue) = BlitzShellProxy::new(event_loop.create_proxy());
    let mut application = BlitzApplication::new(proxy, event_queue);
    application.add_window(WindowConfig::new(
        Box::new(document),
        VelloWindowRenderer::new(),
    ));
    event_loop.run_app(application)?;
    Ok(())
}

fn usage(executable: &str) -> String {
    format!("usage: {executable} [--diagnose] <book.epub>")
}

fn elapsed_ms(start: Instant, end: Instant) -> f64 {
    end.duration_since(start).as_secs_f64() * 1000.0
}

fn process_peak_resident_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")?
            .split_ascii_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

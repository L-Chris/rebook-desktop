# Local egui_commonmark compatibility patch

This directory vendors `egui_commonmark 0.24.0` from crates.io and updates its egui dependencies
to `0.36.0`. The published crate still targets egui 0.35, while Torto needs one shared egui version
across the application and Markdown renderer.

Remove this compatibility patch after an upstream egui_commonmark release supports egui 0.36.

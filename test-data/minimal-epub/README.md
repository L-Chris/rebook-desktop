# Minimal EPUB fixture

Self-authored Phase 0 fixture. It covers EPUB 3 container/package/navigation, an external
stylesheet, relative URLs, Chinese text, an inline link, and a small SVG data image.

Generate the ignored, OCF-conformant `.epub` artifact from the workspace root:

```bash
cargo run -p rebook-inspect --example build_fixture
```

The builder always writes `mimetype` first, stored, and without a trailing newline. Pass an
optional output path after `--` when a fixture is needed elsewhere.

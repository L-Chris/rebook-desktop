# Bundled reader fonts

These font binaries are bundled so the native reader has deterministic Readest-compatible
defaults without depending on fonts installed on the host system.

- Bitter variable Roman and Italic: Google Fonts `ofl/bitter`
- Roboto variable Roman and Italic: Google Fonts `ofl/roboto`
- Fira Code variable: Google Fonts `ofl/firacode`
- LXGW WenKai Regular v1.522: <https://github.com/lxgw/LxgwWenKai/releases/tag/v1.522>

All files are distributed under the SIL Open Font License 1.1. See `OFL-1.1.txt`.
Copyright notices and reserved names remain with their upstream projects:

- Copyright 2011 The Bitter Project Authors, with Reserved Font Name "Bitter Pro".
- Copyright 2011 The Roboto Project Authors.
- Copyright 2014-2020 The Fira Code Project Authors.
- Copyright 2021-2026 LXGW, with the Reserved Font Names declared by the upstream OFL, and
  Copyright 2020 The Klee Project Authors.

The persisted Readest-compatible CJK preference is named `LXGW WenKai GB Screen`; the native
renderer also includes `LXGW WenKai` in its fallback stack because the upstream desktop TTF uses
that family name. The same shared binary is used when a PDF references a non-embedded CJK font.

# MiniCon labs

`lab/` holds bounded, falsifiable experiments. Source, scripts and redacted
results belong in Git; generated executables and target directories do not.
Each lab states what one result can decide and what it cannot.

Current labs:

- `hello-window/` — minimal conventional Rust/Win32 GUI baseline for the 360
  QVM false-positive decision tree.
- `tinygui/` — MiniCon-shaped empty pixel window (same pin and pixel-window
  features). Cross-linked six cells, executed on host/Rosetta/UTM. Receipts
  in `tinygui/RESULTS.md`. One cell is not six-cell.

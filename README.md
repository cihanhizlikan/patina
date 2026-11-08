# Patina

A fast, cross-platform image viewer and editor built in Rust.

Patina is a lightweight, self-contained desktop image viewer and editor focused on speed and a clean workflow. It is built with **egui / eframe** and **wgpu** for GPU-accelerated rendering.

## Features
- **GPU-accelerated rendering** via wgpu, with high-quality **Lanczos zoom**
- **RAW camera support** — reads 20 RAW formats (via `rawler`)
- **Metadata** — EXIF inspection panel
- **XMP sidecar** — star ratings and colour labels written to sidecar files
- **Export presets** — Web, Print and Archive, or your own settings for one save
- **Slideshow mode** and on-screen **guide overlays**
- Cross-platform, with a Windows installer built via Inno Setup

## Tech stack
Rust · eframe / egui · wgpu · rawler

## Building from source
```bash
cargo build --release
```
The release binary will be in `target/release/`.

## Installation (Windows)
A Windows installer is produced with Inno Setup — see the [Releases](../../releases) page.

## License
<!-- Add your license here, e.g. MIT or Apache-2.0 -->

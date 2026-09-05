# Archi

<div align="center">

**A modern, blazing-fast, and secure archive manager crafted for Windows.**  
Open, browse, extract, create, and edit archives with effortless multi-window multitasking and native Drag & Drop.

[![Latest Release](https://img.shields.io/github/v/release/IkEr0228/Archi?style=flat-square&color=38bdf8)](https://github.com/IkEr0228/Archi/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/IkEr0228/Archi/ci.yml?branch=master&style=flat-square&label=CI)](https://github.com/IkEr0228/Archi/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)
[![Platform: Windows](https://img.shields.io/badge/Platform-Windows%2010%20%7C%2011%20x64-0078d4?style=flat-square)](https://github.com/IkEr0228/Archi/releases/latest)
[![Powered by](https://img.shields.io/badge/Stack-Tauri%202%20%2B%20Rust%20%2B%20Svelte%205-ff3e00?style=flat-square)](https://v2.tauri.app/)

<p align="center">
  <a href="#-quick-download"><strong>📥 Download</strong></a> •
  <a href="#-whats-new-in-v040"><strong>✨ What's New</strong></a> •
  <a href="#-features"><strong>⚡ Features</strong></a> •
  <a href="#-keyboard-shortcuts"><strong>⌨️ Shortcuts</strong></a> •
  <a href="#-supported-formats"><strong>📦 Formats</strong></a> •
  <a href="#-build-from-source"><strong>🛠️ Build</strong></a>
</p>

</div>

---

## 📥 Quick Download

Get the latest version of Archi for **Windows 10 / 11 (64-bit)**:

| Distribution | Download Link | Description |
| :--- | :--- | :--- |
| **Installer** *(Recommended)* | [**`archi_0.4.0_x64-setup.exe`**](https://github.com/IkEr0228/Archi/releases/latest) | Complete setup with Start Menu shortcuts and uninstaller. |
| **Portable** | [**`archi.exe`**](https://github.com/IkEr0228/Archi/releases/latest) | Standalone single-file binary. No installation required — run it from anywhere! |

> [!TIP]
> **Windows SmartScreen note:** Since Archi is a free, open-source project without an expensive code-signing certificate, Windows SmartScreen may show a prompt on first run. Simply click **More info** → **Run anyway**.

---

## 📸 Preview

| Main Window (Dark Acrylic) | Create Archive (AES-256) | Password Prompt |
| :---: | :---: | :---: |
| ![Main Window](screens/1.png) | ![Create Archive](screens/2.png) | ![Password Prompt](screens/3.png) |

---

## ✨ What's New in v0.4.0

- 🪟 **Multi-Window Workflow:** Open multiple archives in separate windows side-by-side. Compare contents, organize files, or work across multiple projects simultaneously.
- 🔄 **Cross-Window Drag & Drop:** Drag files directly from one Archi window into another to extract and repack them into a different archive on the fly!
- ⌨️ **Quick Window Creation (`Ctrl + N`):** Launch a clean, independent workspace instantly with `Ctrl + N` or the Titlebar `+` button.
- 📌 **Window Title & Taskbar Sync:** Each window dynamically syncs its title with the open archive's filename across the Windows taskbar and system window previews.
- 🪜 **Smart Cascading Placement:** New windows stagger position automatically so they never obstruct your active workspace.
- 🛡️ **Accidental Close Guard:** Archi prompts before closing any window that has active compression, extraction, or editing tasks underway.

---

## ⚡ Key Features

### 🚀 Blazing Fast & Lightweight
- **High-Performance Core:** Powered by Microsoft's `mimalloc` global memory allocator and hardware-accelerated `ahash` indexing for instant file listing and minimal memory footprint.
- **Virtualized Rendering:** Smooth 60 FPS scrolling through archives containing tens of thousands of entries with Svelte 5 virtual list rendering.
- **Slim Native Binary:** Clean desktop footprint under 10 MB with zero electron bloat.

### 🗂️ Universal Format Compatibility
- **Full Reading & Extraction:** Open, navigate, search, and extract **ZIP**, **7z**, **RAR (RAR4 & RAR5)**, **TAR**, **TAR.GZ**, **TAR.BZ2**, and **TAR.XZ**.
- **Archive Creation:** Create new **ZIP**, **7z (LZMA2)**, and **TAR** archives with configurable compression presets (Store, Fast, Normal, Maximum).
- **Fast In-Place Editing:** Add files to existing ZIP archives with rapid in-place append; enjoy instant deletions in 7z via non-solid **pack-copy** without recompressing entire archives.

### 🖱️ Seamless Drag & Drop
- **Drag Out to Windows Explorer:** Drag files and folders straight out of Archi onto the Desktop, into Explorer folders, Discord, or text editors.
- **Speculative Pre-staging:** Backend decompressions begin speculatively on mouse press (`pointerdown`), delivering near-instant drop responses.
- **Drag In to Pack or Move:** Drop external files into Archi to pack them into the current virtual directory, or drag entries internally between folders.
- **Cross-Window Transfer:** Seamlessly transfer files between multiple open Archi windows.

### 🔒 Safety & Strong Encryption
- **Full AES-256 Encryption:** Open, create, and edit password-protected 7z and ZIP archives. Password-protected RAR archives are fully supported with session password reuse.
- **Zip Bomb & Abuse Heuristics:** Built-in safeguards check expansion ratios (>1000:1), excessive path depths, and suspicious metadata before extraction.
- **Strict Path Traversal Protection:** Absolute paths, drive letters, UNC paths, and dangerous Windows device names (`CON`, `PRN`, `AUX`, `NUL`, etc.) are neutralized.
- **Handle-Relative Native Writes:** Decompression uses secure Windows handle-relative operations to prevent symlink/reparse-point directory escape attacks.

### ⚙️ Windows Explorer Integration
- **1-Click File Associations:** Opt-in per-user (`HKCU`) file associations for `.zip`, `.7z`, `.rar`, `.tar`, and more directly from the **Associations** toolbar dialog. Completely reversible with no administrator rights required.
- **Single-Instance CLI:** Open archives from the command line (`archi.exe file.zip`) or file context menu seamlessly in the existing or new window.

---

## ⌨️ Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| <kbd>Ctrl</kbd> + <kbd>O</kbd> | **Open Archive** dialog |
| <kbd>Ctrl</kbd> + <kbd>N</kbd> | **New Window** |
| <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>N</kbd> | **Create Archive** dialog |
| <kbd>Ctrl</kbd> + <kbd>F</kbd> | Focus archive search bar |
| <kbd>Ctrl</kbd> + <kbd>A</kbd> | Select all entries in current view |
| <kbd>Delete</kbd> | Delete selected files/folders (with confirmation) |
| <kbd>F2</kbd> | Rename selected file or folder |
| <kbd>Backspace</kbd> / <kbd>Alt</kbd> + <kbd>↑</kbd> | Navigate to parent folder |
| <kbd>Enter</kbd> | Open highlighted folder / inspect entry |
| <kbd>Esc</kbd> | Clear selection / close modal dialogs |

---

## 📦 Supported Formats Matrix

| Format | Open & Browse | Extract | Create | In-Archive Edit | Encryption | Notes |
| :---: | :---: | :---: | :---: | :---: | :---: | :--- |
| **ZIP** | ✅ | ✅ | ✅ | ✅ | **AES-256** | Stored + Deflate. Fast append add, logical delete, full compact rebuild. |
| **7z** | ✅ | ✅ | ✅ | ✅ | **AES-256** | LZMA / LZMA2. Non-solid pack-copy (fast delete/move/replace without recompressing). |
| **RAR** | ✅ | ✅ | ❌ | ❌ | **Password** | RAR4 and modern RAR5 support via official `unrar` engine. Read & extract only. |
| **TAR** | ✅ | ✅ | ✅ | ✅ | via .7z | Stream rebuild. Password request creates encrypted `.7z` container. |
| **TAR.GZ** | ✅ | ✅ | ✅ | ✅ | via .7z | Fast stream rebuild. Encrypted create writes `.7z`. |
| **TAR.BZ2** | ✅ | ✅ | ✅ | ✅ | via .7z | Fast stream rebuild. Encrypted create writes `.7z`. |
| **TAR.XZ** | ✅ | ✅ | ✅ | ✅ | via .7z | Fast stream rebuild. Encrypted create writes `.7z`. |
| **GZ / BZ2 / XZ** | ✅ | ✅ | ❌ | ❌ | ❌ | Single-file compressed stream inspection, extraction, and integrity test. |

---

## 🖱️ Drag & Drop Guide

| Source | Destination | Result |
| :--- | :--- | :--- |
| **Archi Table** | Desktop / Windows Explorer / App | **Extracts selected files** to that folder or opens them in the app. |
| **Archi Table** | Another Archi Window | **Copies files directly** into the target archive. |
| **Windows Explorer** | Open Editable Archive | **Adds dropped files** into the current virtual folder. |
| **Windows Explorer** | Internal Folder / Breadcrumb | **Adds files directly** into that subfolder. |
| **Archi Table** | Internal Folder in same window | **Moves selected entries** into that subfolder. |
| **Windows Explorer** | Empty Archi Window | Opens the **Create Archive** dialog with selected paths pre-filled. |

---

## 🛠️ Build from Source

### Prerequisites
- **Operating System:** Windows 10 or 11 (x64)
- **Node.js:** 20+ and `npm`
- **Rust:** Stable toolchain (`x86_64-pc-windows-msvc`)
- **Build Tools:** Visual Studio C++ Build Tools (with Windows SDK)

### Quick Build

```powershell
# 1. Clone repository
git clone https://github.com/IkEr0228/Archi.git
cd Archi

# 2. Install dependencies
npm install

# 3. Run in development mode
npm run tauri dev

# 4. Build optimized production release
npm run tauri build
```

Production artifacts are written to:
- **Portable EXE:** `src-tauri/target/release/archi_backend.exe` (rename to `archi.exe`)
- **NSIS Installer:** `src-tauri/target/release/bundle/nsis/archi_0.4.0_x64-setup.exe`

### Running Quality Checks

```powershell
npm run test:frontend                                      # Frontend unit tests
npm run check                                              # Svelte & TypeScript checks
npm run build                                              # Frontend production build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check  # Rust code format check
cargo test --manifest-path src-tauri/Cargo.toml            # Backend test suite (160+ tests)
```

---

## 🗺️ Documentation

- **[`docs/STATUS.md`](docs/STATUS.md):** Project roadmap, phase progression, and release history.
- **[`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md):** Architecture, coding standards, and security conventions.
- **[`SECURITY.md`](SECURITY.md):** Security policies and vulnerability reporting.
- **[`CONTRIBUTING.md`](CONTRIBUTING.md):** Guidelines for contributing to Archi.

---

## 📄 License

This project is licensed under the [MIT License](LICENSE) © 2026 [IKER](https://github.com/IkEr0228).  
Third-party libraries and crates remain under their respective licenses.

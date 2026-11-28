//! VisorGuBIMGal desktop shell for Windows (E9): Win32 + WebView2, no Tauri.
//!
//! - One instance: a second launch (double-clicking another `.ifc`) hands its files to the
//!   running window through `WM_COPYDATA` and exits.
//! - The app is the web build in the `app` folder next to the executable, served by WebView2's
//!   virtual host `https://app.gubim.localhost/`. `--dev-url <url>` loads the Vite dev server
//!   instead, and `--app-dir <folder>` serves another build folder.
//! - Local IFC files are read through `https://gubim-file.localhost/<token>?start=&end=` in
//!   ranges of at most 16 MiB, so a 184 MB model never crosses the message channel and the
//!   shell's memory stays bounded: WebView2 copies each response several times in this process (E4).
//! - Saving: the page posts the bytes to `https://gubim-file.localhost/__save?name=&filter=&ext=`
//!   and the host asks where with the native save dialog.
//! - Messages `{ id, cmd, args }` from the page: `pickIfcFiles`, `registerIfcPath`,
//!   `takePendingFiles`. Replies `{ id, ok, result | error }`; events `{ event: "open-file", file }`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Off Windows only the tests use it (CI runs them on Linux).
#[cfg_attr(not(windows), allow(dead_code))]
mod logic;

#[cfg(windows)]
mod host;

#[cfg(windows)]
fn main() {
    if let Err(err) = host::run() {
        host::fatal(&format!("No se pudo iniciar VisorGuBIMGal.\n\n{err}"));
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("VisorGuBIMGal escritorio solo funciona en Windows.");
    std::process::exit(1);
}

# Notices

## Project licence

The Rust crates in `crates/` are offered under `MIT OR Apache-2.0`, except for the third-party material listed below. The licence of the rest of this repository is set by GuBIM Galicia, see `README.md`.

## buildingSMART IFC schemas

`crates/ifc-schema/src/tables.rs`, and any build that compiles it, contains entity names, attribute names, attribute order, inheritance and optional and abstract markers taken from these publications:

| Schema | Publication |
|---|---|
| IFC2X3 TC1 | Copyright buildingSMART International Ltd., 1996-2020. All rights reserved. |
| IFC4 ADD2 TC1 | Copyright buildingSMART International Limited, 1996-2020. |
| IFC4X3 ADD2 (IFC 4.3.2) | Copyright buildingSMART International Limited. Licensed under [CC BY-ND 4.0](https://creativecommons.org/licenses/by-nd/4.0/). |

**That material belongs to buildingSMART International Limited and is not covered by this project's licence.** Nothing here grants any right in it.

The tables are a machine translation of those names and structures into Rust, used only to read files in the IFC format. They do not change the specification. Descriptions, rules and functions from the schemas are not included.

The material is provided as is. buildingSMART International Limited gives no warranty for it and is not liable for its use.

## Trademarks

buildingSMART®, Home of openBIM®, openBIM® and IFC™ are (registered) trademarks of buildingSMART International Limited. This project is independent, and is not affiliated with, certified or endorsed by buildingSMART.

## Desktop application (Windows)

`VisorGuBIMGal.exe`, built from `desktop/` and installed with this file next to it, includes:

- `windows`, `windows-core` and related crates (Microsoft, `MIT OR Apache-2.0`) and `webview2-com`, `webview2-com-sys`, `webview2-com-macros` (`MIT`), and `serde`, `serde_json` (`MIT OR Apache-2.0`).
- The WebView2 loader (`WebView2LoaderStatic.lib`) from the Microsoft.Web.WebView2 SDK 1.0.3800.47, linked into the executable. Its licence:

```
Copyright (C) Microsoft Corporation. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * The name of Microsoft Corporation, or the names of its contributors 
may not be used to endorse or promote products derived from this
software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

The application runs on Microsoft Edge WebView2 Runtime, which Windows provides and the installer does not include.

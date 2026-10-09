# Third-party notices

## Nomad Launcher (MIT)

The updater window and pipeline are ported from [Nomad Launcher](https://github.com/cyph3rpuNk-dev/Nomad-Launcher):

| This project | Nomad Launcher source |
| --- | --- |
| `src/ui/mod.rs`, `src/ui/identity.rs`, `src/ui/theme.rs` | `core/src/ui/` |
| `src/taskbar.rs` | `core/src/taskbar.rs` |
| `src/pipeline.rs` | `core/src/lib.rs` (pipeline thread, state helpers) |
| `src/updater.rs` | `core/src/updater.rs` |
| `src/install.rs` | `core/src/install.rs` |
| `src/extract.rs` | `core/src/extract.rs` |
| `src/download.rs` | `core/src/downloader.rs` |
| `src/github.rs` | `core/src/browsers/github.rs`, `core/src/browsers/ungoogled.rs` |

```
MIT License

Copyright (c) 2026 Cyph3rpuNk-dev and Nomad Launcher contributors

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

## Atkinson Hyperlegible (SIL OFL 1.1)

`assets/AtkinsonHyperlegible-Regular.ttf` is embedded as the UI font. License: `assets/OFL.txt`.

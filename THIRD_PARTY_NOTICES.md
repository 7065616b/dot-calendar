# Third-party notices

Dot Calendar uses the following direct Rust dependencies. Versions below are
the versions in the current `Cargo.lock`; transitive dependencies are recorded
there as well. `scripts/package.ps1` includes the resolved Windows dependency
license and notice texts in the preview ZIP's `licenses/` directory, with a
version and license index. Dot Calendar's own source code is licensed under
the MIT License in `LICENSE`. Dependencies retain their respective licenses.

| Crate | Version | Declared license |
| --- | --- | --- |
| `serde` | 1.0.229 | MIT OR Apache-2.0 |
| `serde_json` | 1.0.151 | MIT OR Apache-2.0 |
| `ureq` | 2.12.1 | MIT OR Apache-2.0 |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 |
| `base64` | 0.22.1 | MIT OR Apache-2.0 |
| `getrandom` | 0.2.17 | MIT OR Apache-2.0 |
| `chrono` | 0.4.45 | MIT OR Apache-2.0 |
| [`rs-klc`](https://github.com/chunghha/rs-klc) | 0.2.0 | MIT |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 |

## `rs-klc` 0.2.0 license

The following text is copied from the `LICENSE` included with the
`rs-klc` 0.2.0 source package, including the upstream authors' notices.

```text
The MIT License (MIT)

Copyright (c) 2018 usingsky(usingsky@gmail.com)
Copyright (c) 2022 chunghha(chunghha@users.noreply.github.com)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the \"Software\"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

# PDFium for Android

`arm64-v8a/libpdfium.so` is PDFium build 7881 (chromium/7881) for Android on 64-bit ARM, without V8,
from https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium%2F7881
(`pdfium-android-arm64.tgz`, SHA-256 16d23bb86c4188d59326dc509938f59df3c417ecfdd3d2ca2f160dc5bd49a839):
the same build as the Windows `pdfium.dll` one folder up.

The Android app loads it only to write annotations into a PDF or take them out (crates/pdfium-write);
reading never touches it. `android/build-rust.ps1` copies it into the app beside the engine. There's no
x86-64 copy, so on the emulator a document can be annotated but not saved. PDFium's license and those
of the libraries built into it are in `LICENSE` and `licenses/`.

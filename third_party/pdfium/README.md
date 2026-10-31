# PDFium

`pdfium.dll` is PDFium build 7881 (chromium/7881) for Windows x64, without V8, from
https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium%2F7881 (`pdfium-win-x64.tgz`,
SHA-256 73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08).

Flores PDF loads it only to fill in forms and to save copies with forms and highlights written in;
reading never touches it. The build copies it next to `FloresPDF.exe`. PDFium's license and those of
the libraries built into it are in `LICENSE` and `licenses/`.

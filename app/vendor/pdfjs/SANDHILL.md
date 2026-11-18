# PDF.js in Sandhill

Source: Mozilla PDF.js [v6.5.438](https://github.com/mozilla/pdf.js/releases/tag/v6.5.438), the released [legacy viewer build](https://github.com/mozilla/pdf.js/releases/download/v6.5.438/pdfjs-6.5.438-legacy-dist.zip).

Archive SHA-256: `55138aa33b5f1f6f68809895338b947f395f344eb6381d424b2420d6daa6a844`.

The files are unchanged. Sandhill omits the example PDF, source maps and
standard fonts other than the BSD-licensed Foxit Symbol and Dingbats.
PDF.js uses Linux system fonts for other unembedded standard fonts, so the
GPLv2 Liberation Sans files from the release are not shipped.

Sandhill uses the legacy build for WebKitGTK compatibility. Its toolbar
bridge lives in `app/src/editor/media/linux/`, not in the upstream files.

PDF.js is Apache-2.0 (`LICENSE`). Upstream notices for bundled character
maps, colour profiles, fonts and WASM libraries sit next to those files
under `web/`. `notices/make.sh` copies them into the Linux package notices.

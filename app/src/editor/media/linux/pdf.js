(() => {
  const send = (message) => window.ipc.postMessage(JSON.stringify(message));
  const fail = (message) => send({ type: "error", message: String(message) });

  let initialized = false;
  window.addEventListener("unhandledrejection", (event) => {
    if (!initialized) fail(event.reason?.message ?? event.reason);
  });
  window.addEventListener("error", (event) => {
    if (!initialized && event.target instanceof HTMLScriptElement) {
      fail("PDF.js could not start");
    }
  }, true);

  document.addEventListener("webviewerloaded", () => {
    const app = window.PDFViewerApplication;
    const { ScrollMode } = window.PDFViewerApplicationConstants;
    window.PDFViewerApplicationOptions.setAll({
      disablePreferences: true,
      disableHistory: true,
      viewOnLoad: 1, // INITIAL, not the last position of an earlier document.
      defaultZoomValue: "page-fit",
      scrollModeOnLoad: ScrollMode.PAGE,
      annotationEditorMode: -1, // DISABLE
      annotationMode: 1, // ENABLE, without editable forms.
      enableScripting: false,
      enableXfa: false,
      enableAutoLinking: false,
      useSystemFonts: true,
    });

    const style = document.createElement("link");
    style.rel = "stylesheet";
    style.href = "sandhill.css";
    document.head.append(style);

    // Opening another file or saving through PDF.js would bypass Sandhill's
    // Files pane and its read-only tab.
    window.addEventListener("keydown", (event) => {
      if ((event.ctrlKey || event.metaKey) && ["o", "s", "p"].includes(event.key.toLowerCase())) {
        event.preventDefault();
        event.stopImmediatePropagation();
      }
    }, true);
    window.addEventListener("drop", (event) => {
      event.preventDefault();
      event.stopImmediatePropagation();
    }, true);

    app.initializedPromise.then(() => {
      let loaded = false;
      const position = () => {
        if (!loaded) return;
        const viewer = app.pdfViewer;
        send({
          type: "position",
          pages: app.pagesCount,
          page: app.page,
          scale: viewer.currentScale,
          fit: viewer.currentScaleValue === "page-fit",
        });
      };
      const bus = app.eventBus;
      bus.on("documentinit", () => {
        initialized = loaded = true;
        position();
      });
      bus.on("pagechanging", position);
      bus.on("scalechanging", position);
      bus.on("documenterror", (event) => fail(event.message));
    });
  }, { once: true });
})();

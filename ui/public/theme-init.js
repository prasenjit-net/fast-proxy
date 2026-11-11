// Apply the real default theme before first paint, so there's no
// flash of the wrong mode while the SPA boots and fetches
// /api/config. The React ThemeProvider takes over after hydration.
(function () {
  var mode = null;
  try {
    mode = localStorage.getItem("rt-theme");
  } catch (e) {
    /* storage unavailable */
  }
  if (!mode) {
    var meta = document.querySelector('meta[name="default-theme"]');
    var configured = meta && meta.content;
    mode = configured === "light" || configured === "dark" ? configured : "auto";
  }
  var dark =
    mode === "dark" ||
    (mode !== "light" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
})();

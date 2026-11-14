// Site links in the top bar of the Oxbloom docs.
(function () {
  var root = typeof path_to_root === "string" ? path_to_root : "";
  var bar = document.querySelector("#mdbook-menu-bar .right-buttons");
  if (!bar) return;
  var nav = document.createElement("nav");
  nav.className = "ox-nav";
  nav.setAttribute("aria-label", "Site");
  [
    ["Guide", root + "guide/01-status.html"],
    ["API", root + "API.html"],
    ["Rustdoc", root + "api/oxbloom_boot/index.html"],
    ["GitHub", "https://github.com/pierrejochem/oxbloom", "ox-nav--cta"],
  ].forEach(function (l) {
    var a = document.createElement("a");
    a.href = l[1];
    a.textContent = l[0];
    if (l[2]) a.className = l[2];
    nav.appendChild(a);
  });
  bar.insertBefore(nav, bar.firstChild);
})();

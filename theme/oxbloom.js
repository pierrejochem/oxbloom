// Oxbloom docs chrome: site links in the top bar and a card-style sidebar
// (brand header, icons, collapsible groups, theme switch, project footer).
(function () {
  "use strict";
  var root = typeof path_to_root === "string" ? path_to_root : "";
  var NS = "http://www.w3.org/2000/svg";

  var ICONS = {
    home: '<path d="M3 11l9-8 9 8"/><path d="M5 10v10h14V10"/>',
    activity: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
    clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
    terminal: '<path d="M4 17l6-6-6-6"/><path d="M12 19h8"/>',
    boxes: '<path d="M12 3l8 4.5v9L12 21l-8-4.5v-9z"/><path d="M12 12l8-4.5M12 12v9M12 12L4 7.5"/>',
    sliders: '<path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0"/><circle cx="16" cy="6" r="2"/><circle cx="10" cy="12" r="2"/><circle cx="18" cy="18" r="2"/>',
    refresh: '<path d="M20 11a8 8 0 0 0-14-4L4 9"/><path d="M4 4v5h5"/><path d="M4 13a8 8 0 0 0 14 4l2-2"/><path d="M20 20v-5h-5"/>',
    branch: '<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="9" r="2"/><path d="M6 7v10M18 11c0 4-6 3-12 6"/>',
    file: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5M9 13h6M9 17h6"/>',
    lock: '<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>',
    key: '<circle cx="8" cy="15" r="4"/><path d="M11 12l9-9M16 7l3 3M14 9l2 2"/>',
    shield: '<path d="M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z"/>',
    alert: '<path d="M12 3l10 18H2z"/><path d="M12 10v5M12 18h0"/>',
    braces: '<path d="M8 4c-2 0-3 1-3 3v2c0 1-1 2-2 3 1 1 2 2 2 3v2c0 2 1 3 3 3M16 4c2 0 3 1 3 3v2c0 1 1 2 2 3-1 1-2 2-2 3v2c0 2-1 3-3 3"/>',
    globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18"/>',
    archive: '<rect x="3" y="4" width="18" height="4" rx="1"/><path d="M5 8v11h14V8M10 12h4"/>',
    folder: '<path d="M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
    code: '<path d="M8 8l-5 4 5 4M16 8l5 4-5 4M14 5l-4 14"/>',
    network: '<rect x="9" y="3" width="6" height="5" rx="1"/><rect x="3" y="16" width="6" height="5" rx="1"/><rect x="15" y="16" width="6" height="5" rx="1"/><path d="M12 8v4M6 16v-4h12v4"/>',
    map: '<path d="M9 4L3 6v14l6-2 6 2 6-2V4l-6 2z"/><path d="M9 4v14M15 6v14"/>',
    book: '<path d="M4 5a2 2 0 0 1 2-2h13v16H6a2 2 0 0 0-2 2z"/><path d="M4 19V5"/>',
    pencil: '<path d="M4 20l1-4L16 5l3 3L8 19z"/><path d="M14 7l3 3"/>',
    list: '<path d="M9 6h11M9 12h11M9 18h11M4 6h0M4 12h0M4 18h0"/>',
    chevron: '<path d="M9 6l6 6-6 6"/>',
    left: '<path d="M15 6l-6 6 6 6"/>',
    sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
    moon: '<path d="M20 14.5A8 8 0 0 1 9.5 4 8 8 0 1 0 20 14.5z"/>',
    github: '<path d="M9 19c-4 1.5-4-2-6-2m12 4v-3.5a3 3 0 0 0-1-2.5c3.5-.4 6-1.7 6-6a4.6 4.6 0 0 0-1.3-3.2 4.2 4.2 0 0 0-.1-3.2s-1.1-.3-3.5 1.3a12 12 0 0 0-6.2 0C6.5 2.8 5.4 3.1 5.4 3.1a4.2 4.2 0 0 0-.1 3.2A4.6 4.6 0 0 0 4 9.5c0 4.3 2.5 5.6 6 6a3 3 0 0 0-1 2.5V21"/>',
  };

  function svg(name, cls) {
    var s = document.createElementNS(NS, "svg");
    s.setAttribute("viewBox", "0 0 24 24");
    s.setAttribute("aria-hidden", "true");
    s.setAttribute("class", cls || "ox-ic");
    s.innerHTML = ICONS[name];
    return s;
  }

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text) e.textContent = text;
    return e;
  }

  var BY_FILE = [
    ["introduction", "home"], ["01-status", "activity"], ["02-cli", "terminal"],
    ["03-dependency", "boxes"], ["04-configuration", "sliders"], ["05-application", "refresh"],
    ["06-conditions", "branch"], ["07-logging", "file"], ["08-security", "shield"],
    ["09-errors", "alert"], ["10-json", "braces"], ["11-cors", "globe"],
    ["12-compression", "archive"], ["13-static", "folder"], ["14-example", "code"],
    ["15-http", "network"], ["16-ssl", "shield"], ["17-actuator", "activity"], ["18-tasks", "clock"], ["19-password", "key"], ["20-sessions", "lock"], ["21-form-login", "shield"], ["22-csrf", "shield"], ["23-session-management", "lock"], ["24-remember-me", "key"], ["25-method-security", "shield"], ["26-http-client", "network"], ["29-roadmap", "map"], ["API", "book"], ["ROADMAP-core", "map"],
    ["ROADMAP-modules", "map"],
  ];
  function iconFor(href) {
    for (var i = 0; i < BY_FILE.length; i++) if (href.indexOf(BY_FILE[i][0]) !== -1) return BY_FILE[i][1];
    return "file";
  }

  /* ---- top bar links ---- */
  function topBar() {
    var bar = document.querySelector("#mdbook-menu-bar .right-buttons");
    if (!bar || bar.querySelector(".ox-nav")) return;
    var nav = el("nav", "ox-nav");
    nav.setAttribute("aria-label", "Site");
    [
      ["Guide", root + "guide/01-status.html"],
      ["API", root + "API.html"],
      ["Rustdoc", root + "api/oxbloom_boot/index.html"],
      ["GitHub", "https://github.com/pierrejochem/oxbloom", "ox-nav--cta"],
    ].forEach(function (l) {
      var a = el("a", l[2] || "", l[0]);
      a.href = l[1];
      nav.appendChild(a);
    });
    bar.insertBefore(nav, bar.firstChild);
  }

  /* ---- theme ---- */
  var THEMES = ["light", "rust", "coal", "navy", "ayu"];
  function isDark() { return /\b(coal|navy|ayu)\b/.test(document.documentElement.className); }
  function setTheme(name) {
    var h = document.documentElement;
    THEMES.forEach(function (t) { h.classList.remove(t); });
    h.classList.add(name);
    try { localStorage.setItem("mdbook-theme", name); } catch (e) {}
    sync();
  }
  var syncers = [];
  function sync() { syncers.forEach(function (f) { f(); }); }

  /* ---- sidebar ---- */
  function sidebar() {
    var sb = document.getElementById("mdbook-sidebar");
    var box = sb && sb.querySelector(".sidebar-scrollbox");
    var list = box && box.querySelector("ol.chapter");
    if (!sb || !list || sb.classList.contains("ox-ready")) return !!(sb && sb.classList.contains("ox-ready"));
    sb.classList.add("ox-ready");

    sb.insertBefore(el("div", "ox-card"), sb.firstChild);

    /* header: logo tile, name, collapse */
    var head = el("div", "ox-head");
    var tile = el("a", "ox-tile");
    tile.href = root + "introduction.html";
    tile.setAttribute("aria-label", "Oxbloom home");
    var logo = el("img");
    logo.src = root + "assets/mark.svg";
    logo.alt = "";
    tile.appendChild(logo);
    var name = el("div", "ox-name");
    name.appendChild(el("strong", "", "Oxbloom"));
    name.appendChild(el("span", "", "Developer docs"));
    var collapse = el("button", "ox-collapse");
    collapse.type = "button";
    collapse.title = "Hide navigation";
    collapse.setAttribute("aria-label", "Hide navigation");
    collapse.appendChild(svg("left"));
    collapse.addEventListener("click", function () {
      var t = document.getElementById("mdbook-sidebar-toggle-anchor");
      if (t) t.click();
    });
    head.appendChild(tile); head.appendChild(name); head.appendChild(collapse);
    sb.insertBefore(head, box);

    /* tree: icons, groups, chevrons */
    var kids = Array.prototype.slice.call(list.children);
    var group = null;
    var current = null;
    list.querySelectorAll("a").forEach(function (a) { if (a.classList.contains("active")) current = a; });

    kids.forEach(function (li) {
      if (li.classList.contains("part-title")) {
        var title = li.textContent.trim();
        group = null;
        if (title === "Design specs" || title === "Implementation plans") {
          li.className = "chapter-item ox-parent";
          var btn = el("button", "ox-row ox-group-toggle");
          btn.type = "button";
          btn.setAttribute("aria-expanded", "false");
          btn.appendChild(svg("chevron", "ox-ic ox-chev"));
          btn.appendChild(svg(title === "Design specs" ? "pencil" : "list"));
          btn.appendChild(el("span", "ox-text", title));
          var children = el("ol", "ox-children");
          children.hidden = true;
          li.textContent = "";
          li.appendChild(btn);
          li.appendChild(children);
          btn.addEventListener("click", function () {
            var open = children.hidden;
            children.hidden = !open;
            btn.setAttribute("aria-expanded", String(open));
          });
          group = children;
        } else {
          li.classList.add("ox-label");
          if (title === "Guide") li.classList.add("ox-first");
        }
        return;
      }
      var a = li.querySelector("a");
      if (!a) return;
      if (group) {
        li.classList.add("ox-child");
        group.appendChild(li);
        if (a.classList.contains("active")) {
          group.hidden = false;
          group.parentNode.querySelector(".ox-group-toggle").setAttribute("aria-expanded", "true");
        }
      } else {
        a.classList.add("ox-row");
        var strong = a.querySelector("strong");
        if (strong) strong.remove();
        var text = el("span", "ox-text", a.textContent.trim());
        a.textContent = "";
        a.appendChild(svg("chevron", "ox-ic ox-chev ox-chev-off"));
        a.appendChild(svg(iconFor(a.getAttribute("href") || "")));
        a.appendChild(text);
      }
    });

    /* footer: theme switch and project */
    var foot = el("div", "ox-foot");
    var row = el("div", "ox-switch-row");
    var sw = el("button", "ox-switch");
    sw.type = "button";
    sw.setAttribute("role", "switch");
    sw.setAttribute("aria-label", "Dark theme");
    sw.appendChild(el("span", "ox-knob"));
    row.appendChild(svg("sun", "ox-ic ox-sun"));
    row.appendChild(sw);
    row.appendChild(svg("moon", "ox-ic ox-moon"));
    sw.addEventListener("click", function () { setTheme(isDark() ? "light" : "coal"); });
    syncers.push(function () { sw.setAttribute("aria-checked", String(isDark())); });
    sync();
    var proj = el("a", "ox-project");
    proj.href = "https://github.com/pierrejochem/oxbloom";
    var pi = el("span", "ox-pi");
    pi.appendChild(svg("github"));
    var pt = el("span", "ox-pt");
    pt.appendChild(el("strong", "", "pierrejochem/oxbloom"));
    pt.appendChild(el("span", "", "MIT OR Apache-2.0"));
    proj.appendChild(pi); proj.appendChild(pt);
    foot.appendChild(row); foot.appendChild(proj);
    sb.appendChild(foot);

    if (current) current.scrollIntoView({ block: "nearest" });
    return true;
  }

  function init() {
    topBar();
    if (sidebar()) return;
    var sb = document.getElementById("mdbook-sidebar");
    if (!sb) return;
    var mo = new MutationObserver(function () { if (sidebar()) mo.disconnect(); });
    mo.observe(sb, { childList: true, subtree: true });
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();

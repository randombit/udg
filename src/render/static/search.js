// udg client-side search: loads the build-time index, ranks by
// name-match quality, keyboard-driven ('s' focus, arrows, Enter).
"use strict";

(function () {
  const input = document.getElementById("search");
  const results = document.getElementById("search-results");
  if (!input || !results) return;

  let index = null;
  let indexPromise = null;
  let active = -1;
  let shown = [];

  // Load the index by injecting a <script> that assigns a global —
  // fetch() would be CORS-blocked when the docs are opened via file://.
  // A single shared promise so rapid keystrokes trigger one load.
  function loadIndex() {
    if (index !== null) return Promise.resolve(index);
    if (!indexPromise) {
      indexPromise = new Promise((resolve, reject) => {
        const s = document.createElement("script");
        s.src = UDG_ROOT + "search-index.js";
        s.onload = () => {
          index = window.UDG_SEARCH_INDEX || [];
          resolve(index);
        };
        s.onerror = () => reject(new Error("failed to load search index"));
        document.head.appendChild(s);
      });
    }
    return indexPromise;
  }

  function score(entry, q) {
    const name = entry.n.toLowerCase();
    const qual = entry.q.toLowerCase();
    if (name === q) return 0;
    if (name.startsWith(q)) return 1;
    if (name.includes(q)) return 2;
    if (qual.includes(q)) return 3;
    return -1;
  }

  function run() {
    const q = input.value.trim().toLowerCase();
    if (!q) {
      hide();
      return;
    }
    loadIndex().then(() => {
      const matches = [];
      for (const e of index) {
        const s = score(e, q);
        if (s >= 0) matches.push([s, e]);
      }
      matches.sort(
        (a, b) => a[0] - b[0] || a[1].n.length - b[1].n.length || a[1].q.localeCompare(b[1].q)
      );
      shown = matches.slice(0, 60).map((m) => m[1]);
      renderList();
    });
  }

  function renderList() {
    if (!shown.length) {
      results.innerHTML = "<a><span class='q'>no results</span></a>";
      results.hidden = false;
      active = -1;
      return;
    }
    results.innerHTML = "";
    shown.forEach((e) => {
      const a = document.createElement("a");
      a.href = UDG_ROOT + e.u;
      const chip = document.createElement("span");
      chip.className = "chip since";
      chip.textContent = e.k;
      const q = document.createElement("span");
      q.className = "q";
      q.textContent = e.q;
      const m = document.createElement("span");
      m.className = "m";
      m.textContent = e.m;
      a.append(chip, q, m);
      results.appendChild(a);
    });
    results.hidden = false;
    active = -1;
  }

  function hide() {
    results.hidden = true;
    active = -1;
  }

  function setActive(i) {
    const links = results.querySelectorAll("a");
    if (!links.length) return;
    if (active >= 0) links[active].classList.remove("active");
    active = ((i % links.length) + links.length) % links.length;
    links[active].classList.add("active");
    links[active].scrollIntoView({ block: "nearest" });
  }

  input.addEventListener("input", run);
  input.addEventListener("focus", () => input.value && run());
  input.addEventListener("keydown", (ev) => {
    if (ev.key === "ArrowDown") {
      setActive(active + 1);
      ev.preventDefault();
    } else if (ev.key === "ArrowUp") {
      setActive(active - 1);
      ev.preventDefault();
    } else if (ev.key === "Enter") {
      const links = results.querySelectorAll("a");
      const target = links[active >= 0 ? active : 0];
      if (target && target.href) window.location = target.href;
    } else if (ev.key === "Escape") {
      input.blur();
      hide();
    }
  });

  document.addEventListener("keydown", (ev) => {
    if (ev.key === "s" && document.activeElement !== input && !ev.ctrlKey && !ev.metaKey && !ev.altKey) {
      const tag = document.activeElement && document.activeElement.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
      input.focus();
      input.select();
      ev.preventDefault();
    }
  });

  document.addEventListener("click", (ev) => {
    if (!results.contains(ev.target) && ev.target !== input) hide();
  });

  // Deep link: ?q=botan_cipher_ prefills and runs a search (used by
  // cross-language "family" binding links).
  const q = new URLSearchParams(location.search).get("q");
  if (q) {
    input.value = q;
    input.focus();
    run();
  }
})();

// Sidebar injection: the nav tree ships once per tree as nav.js (a
// script assigning UDG_NAV — fetch() would be CORS-blocked on file://).
// Hrefs in the fragment are site-root-relative; prefix them with this
// page's root and mark the current module's chain.
(function () {
  var el = document.getElementById("sidebar");
  if (!el || typeof UDG_NAV === "undefined") return;
  el.innerHTML = UDG_NAV;
  var links = el.querySelectorAll("a");
  for (var i = 0; i < links.length; i++) {
    links[i].setAttribute("href", UDG_ROOT + links[i].getAttribute("href"));
  }
  var cur = el.getAttribute("data-current");
  if (!cur) return;
  var target = null;
  for (var j = 0; j < links.length; j++) {
    if (links[j].getAttribute("href") === UDG_ROOT + cur) {
      target = links[j];
      break;
    }
  }
  var li = target && target.closest("li");
  if (li) {
    li.classList.add("current");
  }
  while (li) {
    li.classList.add("open");
    li = li.parentElement && li.parentElement.closest("li");
  }
})();

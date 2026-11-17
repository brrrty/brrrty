// brrrty.com: the theme toggle and copy buttons on code blocks. Nothing else.
(function () {
  var root = document.documentElement;
  var dark = window.matchMedia("(prefers-color-scheme: dark)");

  function isDark() {
    var t = root.dataset.theme;
    return t ? t === "dark" : dark.matches;
  }

  var toggle = document.querySelector(".theme-toggle");
  if (toggle) {
    var sync = function () { toggle.setAttribute("aria-pressed", String(isDark())); };
    toggle.hidden = false;
    sync();
    toggle.addEventListener("click", function () {
      var next = isDark() ? "light" : "dark";
      root.dataset.theme = next;
      try { localStorage.setItem("theme", next); } catch (e) {}
      sync();
    });
    dark.addEventListener("change", sync);
  }

  if (!navigator.clipboard) return;
  document.querySelectorAll("pre > code").forEach(function (code) {
    var pre = code.parentNode;
    var box = pre.parentNode.classList.contains("code") ? pre.parentNode : null;
    if (!box) {
      box = document.createElement("div");
      box.className = "code";
      pre.parentNode.insertBefore(box, pre);
      box.appendChild(pre);
    }
    var button = document.createElement("button");
    button.type = "button";
    button.className = "copy";
    button.textContent = "Copy";
    button.setAttribute("aria-live", "polite");
    var timer;
    button.addEventListener("click", function () {
      navigator.clipboard.writeText(code.innerText.replace(/\n$/, "")).then(function () {
        button.textContent = "Copied";
        button.classList.add("is-done");
        clearTimeout(timer);
        timer = setTimeout(function () {
          button.textContent = "Copy";
          button.classList.remove("is-done");
        }, 1600);
      });
    });
    box.classList.add("has-copy");
    box.appendChild(button);
  });
})();

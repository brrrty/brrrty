// Live reload for `brrrty serve`.
//
// Ported from client/reload-client.js of @11ty/eleventy-dev-server 2.1.2
// (MIT, Copyright (c) Zach Leatherman), so a page behaves the same way: a CSS-only change
// swaps the stylesheets in place, a change to the page's own template is patched into the
// DOM with morphdom, anything else reloads the page, and a build error is logged to the
// console while the page keeps showing the last good build.

class Util {
  static pad(num, digits = 2) {
    let zeroes = new Array(digits + 1).join(0);
    return `${zeroes}${num}`.slice(-1 * digits);
  }

  static log(message) {
    Util.output("log", message);
  }
  static error(message) {
    Util.output("error", message);
  }
  static output(type, ...messages) {
    let now = new Date();
    let date = `${Util.pad(now.getUTCHours())}:${Util.pad(
      now.getUTCMinutes()
    )}:${Util.pad(now.getUTCSeconds())}.${Util.pad(now.getUTCMilliseconds(), 3)}`;
    console[type](`[brrrty][${date} UTC]`, ...messages);
  }

  static matchRootAttributes(htmlContent) {
    // morphdom leaves the attributes of <html> alone (it morphs its children only).
    let parsed = new DOMParser().parseFromString(htmlContent, "text/html");
    let parsedDoc = parsed.documentElement;
    let newAttrs = parsedDoc.getAttributeNames();
    let docEl = document.documentElement;
    for (let attr of docEl.getAttributeNames().filter((name) => !newAttrs.includes(name))) {
      docEl.removeAttribute(attr);
    }
    for (let attr of newAttrs) {
      docEl.setAttribute(attr, parsedDoc.getAttribute(attr));
    }
  }

  // Don't patch a <link> whose only difference is the cache-busting parameter being erased.
  static isReloadLinkMatch(from, to) {
    if (from.tagName !== "LINK" || to.tagName !== "LINK") {
      return false;
    }
    let oldWithoutHref = from.cloneNode();
    let newWithoutHref = to.cloneNode();
    oldWithoutHref.removeAttribute("href");
    newWithoutHref.removeAttribute("href");
    if (!oldWithoutHref.isEqualNode(newWithoutHref)) {
      return false;
    }
    let oldUrl = new URL(from.href);
    let newUrl = new URL(to.href);
    let paramName = LiveReload.QUERY_PARAM;
    if (!oldUrl.searchParams.has(paramName) || newUrl.searchParams.has(paramName)) {
      return false;
    }
    oldUrl.searchParams.set(paramName, "");
    newUrl.searchParams.set(paramName, "");
    return oldUrl.toString() === newUrl.toString();
  }

  static fullPageReload() {
    Util.log("Page reload initiated.");
    window.location.reload();
  }
}

class LiveReload {
  static QUERY_PARAM = "_brrrty";

  static css(stylesheets) {
    // A full reload when no stylesheet on this page is one that changed.
    let match = false;
    for (let link of document.querySelectorAll(`link[rel="stylesheet"]`)) {
      if (link.href && stylesheets.includes(new URL(link.href).pathname)) {
        match = true;
      }
    }
    if (!match) {
      Util.fullPageReload();
      return;
    }
    for (let link of document.querySelectorAll(`link[rel="stylesheet"]`)) {
      if (link.href) {
        let url = new URL(link.href);
        url.searchParams.set(this.QUERY_PARAM, Date.now());
        link.href = url.toString();
      }
    }
    Util.log("CSS updated without page reload.");
  }

  static async html(pages) {
    let mine = pages.filter(({ url }) => url === document.location.pathname);
    if (mine.length === 0) {
      Util.fullPageReload();
      return;
    }
    let morphed = false;
    try {
      const { default: morphdom } = await import("./morphdom.js");
      for (let { content } of mine) {
        // Like a reload, minus re-running scripts and page lifecycle events.
        morphed = true;
        morphdom(document.documentElement, content, {
          childrenOnly: true,
          onBeforeElUpdated: function (fromEl, toEl) {
            if (fromEl.nodeName === "SCRIPT" && toEl.nodeName === "SCRIPT") {
              if (toEl.innerHTML !== fromEl.innerHTML) {
                Util.log("JavaScript modified, reload initiated.");
                window.location.reload();
              }
              return false;
            }
            if (fromEl.isEqualNode(toEl)) {
              return false;
            }
            if (Util.isReloadLinkMatch(fromEl, toEl)) {
              return false;
            }
            return true;
          },
          addChild: function (parent, child) {
            // Declarative Shadow DOM.
            if (child.nodeName === "TEMPLATE" && child.hasAttribute("shadowrootmode")) {
              let root = parent.shadowRoot;
              if (root) {
                while (root.firstChild) {
                  root.removeChild(root.firstChild);
                }
              }
              for (let newChild of child.content.childNodes) {
                root.appendChild(newChild);
              }
            } else {
              parent.appendChild(child);
            }
          },
          onNodeAdded: function (node) {
            if (node.nodeName === "SCRIPT") {
              Util.log("JavaScript added, reload initiated.");
              window.location.reload();
            }
          },
          onElUpdated: function (node) {
            // Re-attach custom elements.
            if (customElements.get(node.tagName.toLowerCase())) {
              let placeholder = document.createElement("div");
              node.replaceWith(placeholder);
              requestAnimationFrame(() => {
                placeholder.replaceWith(node);
                placeholder = undefined;
              });
            }
          },
        });
        Util.matchRootAttributes(content);
        Util.log("HTML delta applied without page reload.");
      }
    } catch (e) {
      Util.output("error", "Morphdom error", e);
    }
    if (!morphed) {
      Util.fullPageReload();
    }
  }

  constructor() {
    this.reconnectEventCallback = this.reconnect.bind(this);
  }

  init(options = {}) {
    if (!("WebSocket" in window)) {
      return;
    }
    let { protocol, host } = new URL(document.location.href);
    let socket = new WebSocket(`${protocol.replace("http", "ws")}//${host}/.brrrty/live`);

    socket.addEventListener("message", async (event) => {
      try {
        let data = JSON.parse(event.data);
        if (data.type === "reload") {
          if (data.css) {
            LiveReload.css(data.stylesheets || []);
          } else {
            await LiveReload.html(data.pages || []);
          }
        } else if (data.type === "error") {
          Util.error(`Build error: ${data.message}`);
        } else if (data.type === "connected") {
          // Back after the server went away: the page may be out of date.
          if (options.mode === "reconnect") {
            window.location.reload();
          }
          Util.log("Connected");
        }
      } catch (e) {
        Util.error(`Error parsing ${event.data}: ${e.message}`);
      }
    });

    socket.addEventListener("open", () => {
      this.removeReconnectListeners();
    });

    socket.addEventListener("close", () => {
      Util.log("Disconnected");
      this.addReconnectListeners();
    });
  }

  reconnect() {
    Util.log("Reconnecting…");
    this.init({ mode: "reconnect" });
  }

  addReconnectListeners() {
    this.removeReconnectListeners();
    window.addEventListener("focus", this.reconnectEventCallback);
    window.addEventListener("visibilitychange", this.reconnectEventCallback);
  }

  removeReconnectListeners() {
    window.removeEventListener("focus", this.reconnectEventCallback);
    window.removeEventListener("visibilitychange", this.reconnectEventCallback);
  }
}

new LiveReload().init();

// How Check links reads a result (urlCheckVerdict). The cases are real
// answers from the 2026-10-30 export's addresses.
const vm = require("vm")
const { extract, extractConst } = require("./extract")
const ctx = vm.createContext({ URL })
vm.runInContext(
  ["_URL_NOT_FOUND_TITLE", "_URL_NOT_FOUND_PATH", "_URL_HOME_PATH"].map(extractConst).join("\n") + "\n" +
    ["_urlSameish", "urlCheckVerdict"].map(extract).join("\n"),
  ctx,
)
let failed = 0
const ok = (n, c) => {
  if (!c) failed++
  console.log((c ? "  PASS  " : "  FAIL  ") + n)
}
const v = (r) => {
  const x = ctx.urlCheckVerdict(r)
  return x.cat + (x.why ? ":" + x.why : "")
}
const r = (url, status, redirects, finalUrl, title) => ({ url, status, redirects: redirects || [], finalUrl: finalUrl || null, title: title || null })

ok("answered directly", v(r("https://x.nl/a", 200)) === "ok")
ok("404", v(r("https://x.nl/a", 404)) === "broken:status")
ok("no answer", v({ url: "https://x.nl", status: null, error: "Timed out" }) === "broken:error")
ok("403 is restricted, not broken", v(r("https://x.nl/a", 403)) === "restricted")
ok("a not-found page answering 200", v(r("https://x.nl/a", 200, [], null, "Pagina niet gevonden - X")) === "broken:title")
ok("account page → login through temporary hops is OK",
   v(r("https://www.efteling.com/nl/account", 200, [307, 302, 302, 307], "https://login.efteling.com/nl", "Inloggen - Efteling")) === "ok:temporary")
ok("permanent move to a specific page",
   v(r("https://www.efteling.com/nl/park/reserveer-bezoek", 200, [301], "https://www.efteling.com/nl/tickets")) === "moved")
ok("permanent move to a broader page of the same path",
   v(r("https://global.flixbus.com/bus/kaatsheuvel-efteling", 200, [301], "https://global.flixbus.com/bus")) === "moved:parent")
ok("sent to the home page", v(r("https://x.nl/oude-pagina", 200, [301], "https://www.x.nl/")) === "broken:home")
ok("sent to a language root", v(r("https://x.nl/nl/oude-pagina", 200, [302], "https://x.nl/nl-nl/")) === "broken:home")
ok("a home page staying a home page is not 'sent home'", v(r("http://x.nl", 200, [301], "https://www.x.nl/en")) === "moved")
ok("sent to a not-found address", v(r("https://x.nl/a.pdf", 200, [302], "https://x.nl/sitecore/service/notfound.aspx")) === "broken:notfound")
ok("a dropped query parameter is OK", v(r("https://apps.apple.com/nl/app/x/id1?mt=8", 200, [301], "https://apps.apple.com/nl/app/x/id1")) === "ok")
ok("https / www only is OK", v(r("http://github.com/", 200, [301], "https://github.com/")) === "ok")
ok("a moved page that now 404s is broken", v(r("https://mercure-tilburg.nl/ontdek", 404, [308, 301], "https://www.mercure-tilburg.nl/ontdek", "Page not found")) === "broken:status")


// ── Copy formats ──
const fx = vm.createContext({ URL })
vm.runInContext(
  ["URL_COPY_HEAD"].map(extractConst).join("\n") + "\n" +
    'function esc(s) { return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;") }\n' +
    ["urlCheckTableHtml", "urlCheckMarkdown", "_urlFlatRow", "urlCheckTsv", "urlCheckCsv"].map(extract).join("\n"),
  fx,
)
const rows = [
  { status: "Broken · 404", type: "page", url: "https://x.nl/a?b=1&c=2", goesTo: "",
    uses: [{ token: "qa-12", title: "Tickets | prijzen", url: "https://cm/articles/12" }, { token: "dn-5-7", title: "Parkeren", url: "" }] },
  { status: "Moved · 301", type: "image", url: "https://x.nl/i.png", goesTo: "https://x.nl/new.png", uses: [] },
]
const html = fx.urlCheckTableHtml(rows)
ok("table: a real table with a header", html.startsWith("<table") && html.includes("<th>Goes to</th>") && (html.match(/<tr>/g) || []).length === 3)
ok("table: URLs and items are links, & escaped", html.includes('<a href="https://x.nl/a?b=1&amp;c=2">') && html.includes('<a href="https://cm/articles/12">qa-12</a> Tickets | prijzen'))
ok("table: an item without a URL is plain text", html.includes("<br>dn-5-7 Parkeren"))
const md = fx.urlCheckMarkdown(rows)
ok("markdown: header, separator, one line per row", md.split("\n").length === 4 && md.split("\n")[1].startsWith("| ---"))
ok("markdown: a | inside a cell is escaped", md.includes("Tickets \\| prijzen"))
ok("markdown: items link", md.includes("[qa-12](https://cm/articles/12)"))
const tsv = fx.urlCheckTsv(rows).split("\n")
ok("tsv: five columns everywhere", tsv.every((l) => l.split("\t").length === 5))
const csv = fx.urlCheckCsv(rows)
ok("csv: a field with a comma is quoted", csv.includes('"qa-12 Tickets | prijzen, dn-5-7 Parkeren"'))

console.log(failed ? "\n" + failed + " FAILED" : "\nAll checks passed")
process.exit(failed ? 1 : 0)

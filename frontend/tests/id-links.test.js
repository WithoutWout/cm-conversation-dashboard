// Convert IDs to links: which words and numbers in free text become which
// CM.com link, and what the two copy formats carry. Real functions, fixture
// export. See docs/tools.md.
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = vm.createContext({ console })
vm.runInContext(
  `
  let cmBaseUrl = "https://cm.example/ctx"
  function esc(s) { return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;") }
  ${["buildNodeUrl", "buildItemUrl", "parseIdRefs", "idRefUrl", "idRefToken", "idLinksHtml", "idLinksMarkdown"].map(extract).join("\n")}
  const ARTICLES = new Set([3, 9102, 1234, 150])
  const DIALOGS = new Map([[6055, [7, 123]], [1234, [123, 124]], [42, []], [12, [11]]])
  const TDIALOGS = new Set([97])
  const withData = {
    hasData: true,
    article: (id) => ARTICLES.has(id),
    dialog: (id) => DIALOGS.has(id),
    tdialog: (id) => TDIALOGS.has(id),
    node: (d, n) => (DIALOGS.get(d) || []).includes(n),
  }
  const noData = { hasData: false, article: () => false, dialog: () => false, tdialog: () => false, node: () => false }
  function setBase(b) { cmBaseUrl = b }
  `,
  ctx,
)
const known = vm.runInContext("withData", ctx)
const noData = vm.runInContext("noData", ctx)
const parse = (t, k = known) => ctx.parseIdRefs(t, k)
const refs = (t, k) => Array.from(parse(t, k).parts).filter((p) => typeof p !== "string")
const tokens = (t, k) => refs(t, k).map((r) => r.kind[0] + ":" + r.id + (r.node != null ? "/" + r.node : ""))
const eq = (t, want, k) => assert.deepStrictEqual(tokens(t, k), want, JSON.stringify(t))

// Every Dialog spelling goes to the Dialog, every Article spelling to the Article.
for (const t of ["D-42", "dn-42", "DN42", "DN-#42", "dialog 42", "Dialogs 42", "dialoog 42", "dialogen 42", "dialog: 42", "dialog nr 42", "dialog #42", "Dlg 42"]) {
  eq(t, ["d:42"])
}
eq("D6055", ["d:6055"])
eq("DN6055", ["d:6055"])
eq("d 6055", ["d:6055"])
for (const t of ["Q-3", "QA-3", "qa3", "qa 3", "artikel 3", "artikels 3", "artikelen 3", "article 3", "Articles 3", "art. 3", "faq-3", "a-3", "Q 150", "Q150"]) {
  eq(t, [t.includes("150") ? "a:150" : "a:3"])
}

// A bare number is an Article, when it is one.
eq("zie 9102 voor meer", ["a:9102"])
eq("(9102)", ["a:9102"])
eq("9102.", ["a:9102"])
eq("#9102", ["a:9102"])
assert.deepStrictEqual(Array.from(parse("in 2024 was dat zo").skipped), ["2024"])
eq("in 2024 was dat zo", [])
// With no export to check against, three digits and up are taken at their word.
eq("in 2024 was dat zo", ["a:2024"], noData)
// Small numbers are counts, even when an Article has that number; a prefix says otherwise.
eq("3 tickets", [])
eq("qa-3", ["a:3"])

// The prefix decides over the export, and overlapping numbers stay apart.
eq("artikel 1234 en dialoog 1234", ["a:1234", "d:1234"])
const unknown = refs("D-99999")[0]
assert.strictEqual(unknown.inExport, false)
assert.strictEqual(refs("D-42")[0].inExport, true)
assert.strictEqual(refs("D-42", noData)[0].inExport, null)

// Nodes.
eq("1234-123", ["d:1234/123"])
eq("dn-1234-123", ["d:1234/123"])
eq("D-1234-123", ["d:1234/123"])
eq("1234 node 123", ["d:1234/123"])
eq("Dialog 1234 node 123", ["d:1234/123"])
eq("1234 - node 123", ["d:1234/123"])
eq("1234, node 123", ["d:1234/123"])
eq("1234 knoop 123", ["d:1234/123"])
eq("1234 node 123, 124", ["d:1234/123", "d:1234/124"])
// "node" says it is one even if the export has not heard of it.
assert.strictEqual(refs("7777 node 5")[0].inExport, false)
// A bare N-N has to exist, since ranges look the same.
eq("100-200", [])
assert.deepStrictEqual(Array.from(parse("100-200").skipped), ["100-200"])
eq("12-11", ["d:12/11"])
eq("12-10", [])
// Without an export a small pair is more likely a date than a node.
eq("10-12", [], noData)
eq("1234-123", ["d:1234/123"], noData)
// Articles have no nodes: the Article is linked, the rest stays text.
eq("qa-1234-123", ["a:1234"])
eq("artikel 1234 node 5", ["a:1234"])

// Transactional Dialogs.
eq("TD-97", ["t:97"])
eq("t-97", ["t:97"])
eq("T-dialoog 97", ["t:97"])
eq("transactional dialog 97", ["t:97"])
eq("transactionele dialoog 97", ["t:97"])
eq("D-97", ["t:97"]) // exists only as a Transactional Dialog
eq("D-97", ["d:97"], noData)

// Lists carry the word across.
eq("dialogen 42, 1234 en 6055", ["d:42", "d:1234", "d:6055"])
eq("D-42 and 6055", ["d:42", "d:6055"])
eq("artikelen 3, 150 & 9102", ["a:3", "a:150", "a:9102"])
eq("dialog 42, 5 tickets", ["d:42"]) // 5 is no Dialog
eq("1234,9102", ["a:1234", "a:9102"])

// What is not an ID.
for (const t of [
  "12-11-2024", "2026-11-04", "04-11", "12/11/2024", "12:30", "1,5", "1.234", "3.14",
  "9102%", "9102 euro", "€ 9102", "€9102", "+319102", "06-12345678", "0612345678", "abc9102", "9102abc",
  "e-9102", "Q4 2026", "A4", "D3", "T1", "and", "https://cm.com/articles/9102", "www.x.nl/9102", "a9102@mail.nl",
  "node 123", "9102.5",
]) {
  eq(t, [])
}
// "and" and "end" do not read as a d prefix, but the number behind them is still seen.
eq("foo and 9102", ["a:9102"])

// A worklist as people really write it: descriptions after a dash, nodes after
// a comma, remarks in brackets.
const worklist = {
  hasData: false, article: () => false, dialog: () => false, tdialog: () => false, node: () => false,
}
eq(
  `* 1222 - Tickets ter plaatse
* 1474 - Bezoek reserveren (contextueel antwoord)
* 10691 - Groepsticket onder 20p
* D-4066, node 1340
* D-6042, node 11602 en 11603
* D-6045
* D-6163, node 8 (+ contextuele antwoorden)
* D-6321 (helemaal)`,
  ["a:1222", "a:1474", "a:10691", "d:4066/1340", "d:6042/11602", "d:6042/11603", "d:6045", "d:6163/8", "d:6321"],
  worklist,
)

// The text around the links survives untouched.
const t = "Check D-42\nen <b>artikel</b> 9102 & meer"
const { parts } = parse(t)
assert.strictEqual(Array.from(parts).map((p) => (typeof p === "string" ? p : p.text)).join(""), t)

// URLs.
assert.strictEqual(ctx.idRefUrl(refs("qa-9")[0]), "https://cm.example/ctx/articles/9")
assert.strictEqual(ctx.idRefUrl(refs("D-42")[0]), "https://cm.example/ctx/dialogs/42")
assert.strictEqual(ctx.idRefUrl(refs("1234-123")[0]), "https://cm.example/ctx/dialogs/1234?currentNode=123")
assert.strictEqual(ctx.idRefUrl(refs("TD-97")[0]), "https://cm.example/ctx/dialogs/transactional/97")
assert.strictEqual(ctx.idRefToken(refs("dialog 1234 node 123")[0]), "dn-1234-123")

// Copies: HTML for Teams / Jira / TinyMCE, Markdown for plain fields. The link
// reads as typed; lines stay lines.
const copyParts = parse("Zie Dialoog 42\n<en> qa-9").parts
assert.strictEqual(
  ctx.idLinksHtml(copyParts),
  'Zie <a href="https://cm.example/ctx/dialogs/42">Dialoog 42</a><br>&lt;en&gt; <a href="https://cm.example/ctx/articles/9">qa-9</a>',
)
assert.strictEqual(
  ctx.idLinksMarkdown(copyParts),
  "Zie [Dialoog 42](https://cm.example/ctx/dialogs/42)\n<en> [qa-9](https://cm.example/ctx/articles/9)",
)
// Bullets typed as text become a real list; Markdown already reads them as one.
const list = parse("Graag:\n* 9102 - Tickets\n* D-42\nKlaar").parts
assert.strictEqual(
  ctx.idLinksHtml(list),
  'Graag:<br><ul><li><a href="https://cm.example/ctx/articles/9102">9102</a> - Tickets</li><li><a href="https://cm.example/ctx/dialogs/42">D-42</a></li></ul>Klaar',
)

// No Context URL: nothing to link to, so no dead links in either format.
ctx.setBase("")
assert.strictEqual(ctx.idLinksMarkdown(copyParts), "Zie Dialoog 42\n<en> qa-9")
assert.strictEqual(ctx.idLinksHtml(copyParts), "Zie Dialoog 42<br>&lt;en&gt; qa-9")

console.log("id-links ok")

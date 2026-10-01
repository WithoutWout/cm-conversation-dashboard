// Share Content carries a Dialog's matched nodes, each with its own link —
// `…/dialogs/D?currentNode=N` — in every copy format, and an Article's matched
// Responses rather than its default one. The real functions are run against a
// stubbed `contentMatchInfo`, the worker's count (search-match-count.test.js
// pins how that is made).
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = vm.createContext({ console })
vm.runInContext(
  `
  let gQuery = "korting"
  let cmBaseUrl = "https://cm.example/ctx"
  let contentMatchInfo = new Map()
  let _exportView = "list"
  let ITEMS = []
  let copied = null
  const navigator = { clipboard: { writeText: (t) => { copied = t; return Promise.resolve() } } }
  const ClipboardItem = undefined
  const _exportDroppedNodes = new Set()
  function getExportItemsForCurrentView() { return ITEMS.map(_exportWithoutDroppedNodes) }
  function copyRichHtml(h) { copied = h }
  function execCommandCopy(h) { copied = h }
  function _setBtnCopied() {}
  function _showCopyFeedback() {}
  function _exportRelationText() { return "" }
  function _buildModalTermRegexes() { return [/korting/i] }
  function _nodeReasonText(n) { return "says korting in " + n.name }
  function stripDisplay(t) { return t }
  function buildSearchRegex() { return /korting/i }
  function _getMatchReason() { return "(matched on: entity)" }
  const _exportNodesCache = new WeakMap()
  // Not extracted: an earlier \`function esc(\` in index.html is found first.
  function esc(s) {
    return String(s == null ? "" : s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;")
  }
  ${[
    "_matchKey", "contentMatchFor", "_matchCountText", "_matchTotalsText",
    "_defaultAnswerAmong", "defaultArticleAnswer", "buildItemUrl", "buildNodeUrl",
    "_itemIdToken", "exportGroupLabel", "_exportMatchedNodes", "_getMatchedResponses",
    "_exportNodeTotal", "_exportNodeCountText", "_exportWithoutDroppedNodes", "_copyExportLinks", "_copyExportPlain",
    "_copyExportTable",
  ].map(extract).join("\n")}
  globalThis.T = {
    set(info) { contentMatchInfo = new Map(Object.entries(info)) },
    noUrl() { cmBaseUrl = "" },
    nodes: _exportMatchedNodes,
    responses: _getMatchedResponses,
    totals: _matchTotalsText,
    copy(fn, items, view) { ITEMS = items; _exportView = view || "list"; copied = null; fn(null); return copied },
    links: (items, view) => T.copy(_copyExportLinks, items, view),
    plain: (items, view) => T.copy(_copyExportPlain, items, view),
    table: (items) => T.copy(_copyExportTable, items),
    dropNode: (token) => _exportDroppedNodes.add(token),
    restoreNodes: () => _exportDroppedNodes.clear(),
    view: (items) => { ITEMS = items; return getExportItemsForCurrentView() },
    countText: _exportNodeCountText,
  }
  `,
  ctx,
)
const T = ctx.T

const DIALOG = {
  id: 42,
  _kind: "dialog",
  name: "Tarieven",
  description: "",
  nodes: [
    { id: 1, name: "Menu" },
    { id: 2, name: "Kinderen" },
    { id: 3, name: "Jaarkaart" },
  ],
}
const ARTICLE = {
  Id: 7,
  Questions: [{ Text: "korting", IsFaq: true }],
  Outputs: [
    { Type: "Answer", IsDefault: true, Text: "Geen korting." },
    { Type: "Answer", IsDefault: false, Text: "Leden krijgen korting." },
    { Type: "Answer", IsDefault: false, Text: "Iets anders." },
  ],
}
T.set({ "dialog:42": { n: [1, 2], h: false }, "article:7": { q: [0], r: [0, 1] } })

const row = (item, kind) => ({
  kind,
  id: kind === "article" ? item.Id : item.id,
  title: kind === "article" ? "korting" : item.name,
  responses: T.responses(item),
  nodes: kind === "article" ? [] : T.nodes(item),
  _raw: item,
})

let failed = 0
function test(name, fn) {
  try {
    fn()
    console.log("  PASS  " + name)
  } catch (e) {
    failed++
    console.log("  FAIL  " + name + "\n        " + e.message)
  }
}

test("a Dialog carries only its counted nodes, each with its own deep link", () => {
  const nodes = T.nodes(DIALOG)
  assert.deepStrictEqual(
    nodes.map((n) => [n.token, n.name, n.url]),
    [
      ["dn-42-2", "Kinderen", "https://cm.example/ctx/dialogs/42?currentNode=2"],
      ["dn-42-3", "Jaarkaart", "https://cm.example/ctx/dialogs/42?currentNode=3"],
    ],
  )
})

test("a Dialog with listed nodes has no Responses column of its own", () => {
  assert.strictEqual(T.responses(DIALOG).length, 0)
})

test("an Article lists its matched Responses, contextual ones marked", () => {
  assert.deepStrictEqual(T.responses(ARTICLE), ["Geen korting.", "(contextual) Leden krijgen korting."])
})

test("Copy links puts every node on its own line with its own link", () => {
  const html = T.links([row(ARTICLE, "article"), row(DIALOG, "dialog")])
  assert.ok(html.includes('<a href="https://cm.example/ctx/dialogs/42">dn-42</a> → Tarieven'))
  assert.ok(html.includes('↳ <a href="https://cm.example/ctx/dialogs/42?currentNode=2">dn-42-2</a> → Kinderen'))
  assert.ok(html.includes('<a href="https://cm.example/ctx/dialogs/42?currentNode=3">dn-42-3</a> → Jaarkaart'))
  assert.ok(!html.includes("currentNode=1"), "node 1 was not counted")
})

test("Copy as plain text gives each node a line with its url", () => {
  const lines = T.plain([row(DIALOG, "dialog")]).split("\n")
  assert.strictEqual(lines.length, 3)
  assert.ok(lines[1].startsWith("    ↳ dn-42-2\tKinderen\t"))
  assert.ok(lines[1].endsWith("\thttps://cm.example/ctx/dialogs/42?currentNode=2"))
})

test("Copy table makes each node a row of its own, linked", () => {
  const html = T.table([row(DIALOG, "dialog")])
  assert.strictEqual((html.match(/<tr>/g) || []).length, 4) // header, Dialog, 2 nodes
  assert.ok(html.includes('<td><a href="https://cm.example/ctx/dialogs/42?currentNode=3">↳ dn-42-3</a></td><td>Jaarkaart</td>'))
  assert.ok(html.includes("<td>2 of 3 nodes match</td>"))
})

test("without a Context URL the nodes are still listed, as plain ids", () => {
  T.noUrl()
  const html = T.links([row(DIALOG, "dialog")])
  assert.ok(html.includes("↳ dn-42-2 → Kinderen"))
  assert.ok(!html.includes("<a "))
})

test("the result count sums nodes and Responses, contextual ones apart", () => {
  const items = [ARTICLE, DIALOG]
  assert.strictEqual(T.totals([0, 1], (i) => items[i]), " · 2 nodes · 2 responses (1 contextual)")
  T.set({})
  assert.strictEqual(T.totals([0, 1], (i) => items[i]), "")
})

test("a node removed by hand leaves the list and every copy, its Dialog stays", () => {
  T.set({ "dialog:42": { n: [1, 2], h: false } })
  const items = [row(DIALOG, "dialog")]
  T.dropNode("dn-42-2")
  const shown = T.view(items)
  assert.strictEqual(shown.length, 1)
  assert.strictEqual(JSON.stringify(shown[0].nodes.map((n) => n.token)), '["dn-42-3"]')
  assert.strictEqual(T.countText(shown[0]), "1 of 3 nodes match · 1 removed")
  for (const out of [T.links(items), T.plain(items), T.table(items)]) {
    assert.ok(!out.includes("dn-42-2"), "dropped node still copied")
    assert.ok(out.includes("dn-42-3"))
  }
  assert.strictEqual(items[0].nodes.length, 2, "the cached node list was changed")
  T.restoreNodes()
})

test("a Dialog with every matched node removed says so and stays a result", () => {
  T.set({ "dialog:42": { n: [1, 2], h: false } })
  const items = [row(DIALOG, "dialog")]
  T.dropNode("dn-42-2")
  T.dropNode("dn-42-3")
  const shown = T.view(items)
  assert.strictEqual(shown.length, 1)
  assert.strictEqual(T.countText(shown[0]), "all 2 matched nodes removed")
  const html = T.links(items)
  assert.ok(/dn-42(<\/a>)? → Tarieven/.test(html), "the Dialog's own line is gone")
  assert.ok(!html.includes("dn-42-"), "a removed node is still copied")
  T.restoreNodes()
})

console.log(failed ? "\n" + failed + " failing" : "Share Content nodes: all checks passed")
process.exit(failed ? 1 : 0)

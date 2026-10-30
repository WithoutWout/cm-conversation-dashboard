// Share Content's linked content: an Article says which Dialog it starts, a
// Dialog which Articles its nodes show and which Dialogs they start — on every
// row and in every copy, and not at all with the toggle off. The real functions
// run against hand-built content in the export's own shapes.
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = vm.createContext({ console })
vm.runInContext(
  `
  let cmBaseUrl = "https://cm.example/ctx"
  let _exportView = "list"
  let _exportFilter = ""
  let _exportLinkMode = "matching"
  let _exportIncomingCache = null
  let allArticles = []
  let allDialogsCombined = []
  let allCombinedItems = []
  function buildNodeUrl(d, n) { return cmBaseUrl ? cmBaseUrl + "/dialogs/" + d + "?currentNode=" + n : "" }
  let contentMatched = null
  let ITEMS = []
  let copied = null
  const navigator = { clipboard: { writeText: (t) => { copied = t; return Promise.resolve() } } }
  const ClipboardItem = undefined
  const _exportDropped = new Set()
  const _exportDroppedNodes = new Set()
  const articleMap = new Map([[9102, { Id: 9102, _faqQ: "Hoe laat gaat het park open?" }]])
  const dialogMap = new Map([[6055, { id: 6055, name: "Tickets wijzigen" }], [42, { id: 42, name: "Tarieven" }]])
  const tDialogMap = new Map([[7, { id: 7, name: "Retour aanvragen" }]])
  function aFaqQ(a) { return a._faqQ }
  function getActiveExportItems() { return ITEMS }
  function _exportItemMatches() { return true }
  function copyRichHtml(h) { copied = h }
  function execCommandCopy(h) { copied = h }
  function _setBtnCopied() {}
  function _showCopyFeedback() {}
  function openLink() {}
  function esc(s) {
    return String(s == null ? "" : s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;")
  }
  ${[
    "buildItemUrl", "_itemIdToken", "exportGroupLabel", "exportGroupOrder", "groupedExportItems",
    "_exportKey", "_exportWithoutDroppedNodes", "_exportLinksOfMatchedNodes", "_exportWithLinkMode", "_exportIncomingIndex", "_exportLinkArrow", "_exportWithoutListedLinks", "getExportItemsForCurrentView",
    "articleAnswerHasContext", "dialogAnswerHasContext", "_exportItemLinks", "_exportLinkLabel", "_exportLinkNote", "_exportLinkTotal",
    "_exportNodeTotal", "_exportNodeCountText", "_exportLinkTip", "_exportLinksHtml",
    "_copyExportLinks", "_copyExportPlain", "_copyExportTable",
  ].map(extract).join("\n")}
  const _EXPORT_LINK_ORDER = { dialog: 0, tdialog: 1, article: 2 }
  const _exportLinksCache = new WeakMap()
  globalThis.T = {
    links: _exportItemLinks,
    html: _exportLinksHtml,
    setLinks(on) { _exportLinkMode = on ? "matching" : "off" },
    setMode(m) { _exportLinkMode = m },
    setContent(arts, dialogs) { allArticles = arts; allDialogsCombined = dialogs; allCombinedItems = [...arts, ...dialogs] },
    setMatched(keys) { contentMatched = keys ? new Set(keys) : null },
    view(items) { ITEMS = items; return getExportItemsForCurrentView() },
    copy(fn, items) { ITEMS = items; copied = null; fn(null); return copied },
    rich: (items) => T.copy(_copyExportLinks, items),
    plain: (items) => T.copy(_copyExportPlain, items),
    table: (items) => T.copy(_copyExportTable, items),
  }
  `,
  ctx,
)
const T = ctx.T

// The Article starts Dialog 6055 by default and Transactional Dialog 7 only
// under a context condition.
const ARTICLE = {
  Id: 1,
  Outputs: [
    { Type: "DialogStart", DialogId: 6055, IsDefault: true },
    { Type: "TDialogStart", TDialogId: 7, IsDefault: false, ContextVariables: [{ Id: 1, Values: ["nl"] }] },
    { Type: "DialogStart", DialogId: 6055, IsDefault: false, ContextVariables: [{ Id: 1, Values: ["en"] }] },
  ],
}
// Two nodes show Article 9102, one starts Dialog 6055, one starts itself.
const DIALOG = {
  id: 42,
  _kind: "dialog",
  name: "Tarieven",
  nodes: [
    { id: 1, name: "Menu", output: { kbaIdReference: 9102, items: [] } },
    { id: 2, name: "Kinderen", output: { kbaIdReference: 9102, items: [] } },
    { id: 3, name: "Wijzigen", output: { items: [{ type: "DialogStart", isDefault: false, data: { dialogId: 6055 } }] } },
    { id: 4, name: "Opnieuw", output: { items: [{ type: "DialogStart", isDefault: true, data: { dialogId: 42 } }] } },
  ],
}
const row = (raw, kind, title) => ({
  kind,
  id: kind === "article" ? raw.Id : raw.id,
  title,
  responses: [],
  nodes: [],
  links: T.links(raw, kind),
  _raw: raw,
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

test("an Article links to the Dialog it starts, default first, contextual marked", () => {
  const links = T.links(ARTICLE, "article")
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(links.map((l) => [l.token, l.kind, l.title, l.contextual]))),
    [
      ["dn-6055", "dialog", "Tickets wijzigen", false],
      ["dn-7", "tdialog", "Retour aanvragen", true],
    ],
  )
  assert.strictEqual(links[0].url, "https://cm.example/ctx/dialogs/6055")
  assert.strictEqual(links[1].url, "https://cm.example/ctx/dialogs/transactional/7")
})

test("a Dialog links to the Articles its nodes show and the Dialogs they start, once each", () => {
  const links = T.links(DIALOG, "dialog")
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(links.map((l) => [l.token, l.title, JSON.stringify(l.via)]))),
    [
      ["dn-6055", "Tickets wijzigen", '["Wijzigen"]'],
      ["qa-9102", "Hoe laat gaat het park open?", '["Menu","Kinderen"]'],
    ],
  )
  // A node whose only output is the Dialog Link is its default, flag or not.
  assert.strictEqual(links[0].contextual, false)
})

test("the row shows one line per kind with the targets as chips", () => {
  const html = T.html(row(ARTICLE, "article", "Tickets"))
  assert.ok(html.includes("&#8594; Dialog Link</span>"))
  assert.ok(html.includes("&#8594; Transactional Dialog</span>"))
  assert.ok(html.includes('class="export-link-chip tdialog contextual"'))
  assert.ok(html.includes('<span class="export-link-name">Tickets wijzigen</span>'))
  const d = T.html(row(DIALOG, "dialog", "Tarieven"))
  assert.ok(d.includes("&#8594; Article</span>"))
  assert.ok(d.includes("from Menu, Kinderen"), "the tooltip names the nodes")
})

test("Copy links gives each link its own line: its id, linked, and its title — nothing else", () => {
  const html = T.rich([row(ARTICLE, "article", "Tickets")])
  const lines = html.split("<br>")
  assert.strictEqual(lines.length, 3)
  assert.strictEqual(lines[1], '\u00a0\u00a0\u00a0\u00a0→ <a href="https://cm.example/ctx/dialogs/6055">dn-6055</a> → Tickets wijzigen')
  assert.strictEqual(lines[2], '\u00a0\u00a0\u00a0\u00a0→ <a href="https://cm.example/ctx/dialogs/transactional/7">dn-7</a> → Retour aanvragen')
})

test("Copy as plain text and Copy table carry the links in their own columns", () => {
  const lines = T.plain([row(DIALOG, "dialog", "Tarieven")]).split("\n")
  assert.strictEqual(lines.length, 3)
  assert.strictEqual(
    lines[2],
    "    → qa-9102\tHoe laat gaat het park open?\t\thttps://cm.example/ctx/articles/9102",
  )
  const table = T.table([row(DIALOG, "dialog", "Tarieven")])
  assert.strictEqual((table.match(/<tr>/g) || []).length, 4) // header, Dialog, 2 links
  assert.ok(table.includes('<td><a href="https://cm.example/ctx/dialogs/6055">→ dn-6055</a></td><td>Tickets wijzigen</td><td></td>'))
  for (const out of [T.rich([row(ARTICLE, "article", "T")]), lines.join("\n"), table])
    assert.ok(!/Dialog Link|Transactional Dialog|contextual|from Menu/.test(out), "a copy carries a label or a note")
})

test("a link to an item already in the share is left out, and only noted on screen", () => {
  // Dialog 6055 is a row of its own, so neither the Article's Dialog Link to
  // it nor the Dialog's node starting it is repeated.
  const target = { id: 6055, _kind: "dialog", name: "Tickets wijzigen", nodes: [] }
  const items = [row(ARTICLE, "article", "Tickets"), row(DIALOG, "dialog", "Tarieven"), row(target, "dialog", "Tickets wijzigen")]
  const shown = T.view(items)
  const art = shown.find((i) => i.kind === "article")
  const dlg = shown.find((i) => i.id === 42)
  assert.deepStrictEqual(art.links.map((l) => l.token).join(), "dn-7")
  assert.deepStrictEqual(dlg.links.map((l) => l.token).join(), "qa-9102")
  assert.strictEqual(art.linksInShare.length, 1)
  assert.ok(T.html(art).includes("+1 linked item already in this share"))
  for (const out of [T.rich(items), T.plain(items), T.table(items)]) {
    assert.ok(!/→ (Dialog Link )?(<a [^>]*>)?dn-6055/.test(out), "a listed item was copied as a link")
    assert.ok(out.includes("dn-6055"), "the item's own row is still there")
  }
  assert.strictEqual(items[0].links.length, 2, "the cached link list was changed")
  // Removed from the share, it is a link again.
  assert.strictEqual(T.view(items.slice(0, 2))[0].links.length, 2)
})

test("a Dialog matched node by node brings only those nodes' links", () => {
  // The search matched "Wijzigen" (node 3) and "Menu" (node 1), not "Kinderen".
  const matched = (ids) => ({ ...row(DIALOG, "dialog", "Tarieven"), nodes: ids.map((id) => ({ id, token: "dn-42-" + id })) })
  let [d] = T.view([matched([3])])
  assert.strictEqual(d.links.map((l) => l.token).join(), "dn-6055", "an Article only an unmatched node shows was kept")
  ;[d] = T.view([matched([1])])
  assert.strictEqual(d.links.map((l) => l.token).join(), "qa-9102")
  assert.strictEqual(JSON.stringify(d.links[0].via), '["Menu"]', "the tooltip names a node the search did not match")
  // Every matched node removed by hand takes every link with it.
  ;[d] = T.view([{ ...matched([]), nodesRemoved: 2 }])
  assert.strictEqual(d.links.length, 0)
  // Matched as a whole — by its name, its id, or with no search — it keeps them all.
  ;[d] = T.view([matched([])])
  assert.strictEqual(d.links.length, 2)
})

test("a link stays only when its target matches the search too", () => {
  // Searching "albert heijn": the Article and Dialog 6055 mention it, the
  // Transactional Dialog 7 does not — nothing there to edit.
  T.setMatched(["article:1", "dialog:6055"])
  const [art] = T.view([row(ARTICLE, "article", "Tickets")])
  assert.strictEqual(art.links.map((l) => l.token).join(), "dn-6055")
  assert.strictEqual(art.linksInShare, undefined, "a target that does not match is not 'already in this share'")
  for (const out of [T.rich([row(ARTICLE, "article", "T")]), T.plain([row(ARTICLE, "article", "T")])])
    assert.ok(!out.includes("dn-7"), "a link to content the search did not match was copied")
  // Listed as a row as well, it is noted rather than repeated: nothing is left.
  const target = { id: 6055, _kind: "dialog", name: "Tickets wijzigen", nodes: [] }
  const shown = T.view([row(ARTICLE, "article", "Tickets"), row(target, "dialog", "Tickets wijzigen")])
  assert.strictEqual(shown[0].links.length, 0)
  assert.strictEqual(shown[0].linksInShare.length, 1)
  T.setMatched(null)
})

test("the complete overview lists everything that links into a row — matching or not", () => {
  // Dialog 6055 is opened by the Article by default and started by node 3
  // of Dialog 42. Neither has to match the search to depend on it.
  T.setContent([ARTICLE], [DIALOG])
  T.setMode("complete")
  T.setMatched(["dialog:6055"])
  const target = { id: 6055, _kind: "dialog", name: "Tickets wijzigen", nodes: [] }
  let [d] = T.view([row(target, "dialog", "Tickets wijzigen")])
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(d.links.map((l) => [l.token, l.title, l.url]))),
    [
      ["qa-1", "", "https://cm.example/ctx/articles/1"],
      ["dn-42-3", "Tarieven › Wijzigen", "https://cm.example/ctx/dialogs/42?currentNode=3"],
    ],
  )
  assert.ok(T.html(d).includes("&#8592; Linked from Article</span>"))
  assert.ok(T.html(d).includes("&#8592; Linked from Dialog node</span>"))
  const plain = T.plain([row(target, "dialog", "Tickets wijzigen")]).split("\n")
  assert.strictEqual(plain[2], "    ← dn-42-3\tTarieven › Wijzigen\t\thttps://cm.example/ctx/dialogs/42?currentNode=3")
  assert.ok(T.rich([row(target, "dialog", "T")]).includes('← <a href="https://cm.example/ctx/articles/1">qa-1</a>'))
  // The Article as a row is not repeated, nor is Dialog 42 listed whole; a
  // Dialog row that lists other nodes still needs node 3 named.
  d = T.view([row(target, "dialog", "T"), row(ARTICLE, "article", "A"), row(DIALOG, "dialog", "Tarieven")]).find((i) => i.id === 6055)
  assert.strictEqual(d.links.length, 0, "the Article and Dialog 42, listed whole, are both rows already")
  assert.strictEqual(d.linksInShare.length, 2)
  d = T.view([row(target, "dialog", "T"), { ...row(DIALOG, "dialog", "Tarieven"), nodes: [{ id: 1, token: "dn-42-1" }] }]).find((i) => i.id === 6055)
  assert.strictEqual(d.links.map((l) => l.token).join(), "qa-1,dn-42-3", "a listed Dialog that does not list node 3 still needs it named")
  T.setMode("matching")
  T.setMatched(null)
})

test("with Linked content off, no copy carries a link", () => {
  const items = [row(ARTICLE, "article", "Tickets"), row(DIALOG, "dialog", "Tarieven")]
  T.setLinks(false)
  assert.ok(T.view(items).every((i) => i.links.length === 0))
  for (const out of [T.rich(items), T.plain(items), T.table(items)]) {
    assert.ok(!out.includes("6055"), "a link was copied with the toggle off")
    assert.ok(!out.includes("9102"))
  }
  assert.strictEqual(items[0].links.length, 2, "the cached link list was changed")
  T.setLinks(true)
})

console.log(failed ? "\n" + failed + " failing" : "Share Content links: all checks passed")
process.exit(failed ? 1 : 0)

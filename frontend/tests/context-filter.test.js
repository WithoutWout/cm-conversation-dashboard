// Content-side context filtering: the Context tab of the tag popover.
//
// A context condition sits on an *output*, and every output type carries one —
// an Answer, and equally a DialogStart or TDialogStart routing into a Dialog.
// The filter used to read Answers only, so on the real export
// `available_livechat_theater = any` returned 1 item out of 14: the condition is
// set on 11 DialogStart outputs of four Articles, and on the routes of ten
// Dialogs, and on exactly one Answer. Article 737 is the canonical case and the
// fixture below uses its shape.
//
// The worker is driven through its real `init` and `search` messages, so the
// precompute pass and the matcher under test are the ones that run. The chip
// counts from `buildContentContextOptions` (index.html) are then held against
// what those searches return — over the real export when one is available.
const { extract } = require("./extract")
const fs = require("fs")
const path = require("path")
const vm = require("vm")

const ROOT = path.join(__dirname, "..", "..")

const out = []
let failed = 0
const ok = (n, c) => {
  if (!c) failed++
  out.push((c ? "  PASS  " : "  FAIL  ") + n)
}
const eq = (n, a, b) => {
  const same = JSON.stringify(a) === JSON.stringify(b)
  if (!same)
    out.push(`         got ${JSON.stringify(a)} want ${JSON.stringify(b)}`)
  ok(n, same)
}

// ── The worker, whole ────────────────────────────────────────────────────────
const workerSrc = fs.readFileSync(
  path.join(__dirname, "..", "search-worker.js"),
  "utf8",
)
function loadWorker() {
  let last = null
  const self = { postMessage: (m) => (last = m) }
  const wctx = vm.createContext({ console, self })
  vm.runInContext(
    workerSrc +
      "\n;globalThis.__outputCtxSet = outputCtxSet" +
      "\n;globalThis.__setCtxVars = (vars) => { ctxVarMap = new Map(vars.map((v) => [v.id, v.name])) }",
    wctx,
  )
  const send = (msg) => {
    self.onmessage({ data: msg })
    return last
  }
  return { wctx, send }
}

/** Every item a set of filters returns, as "a<Id>" / "d<id>" keys. */
function searcher(articles, dialogs, ctxVars) {
  const { send } = loadWorker()
  send({
    type: "init",
    json: JSON.stringify({
      articles: articles.map((a) => ({ ...a, _kind: "article" })),
      dialogs: dialogs.map((d) => ({ ...d, _kind: "dialog" })),
      entities: [],
      convVars: [],
      ctxVars,
    }),
  })
  const keys = [
    ...articles.map((a) => "a" + a.Id),
    ...dialogs.map((d) => "d" + d.id),
  ]
  return (filters, query = "") => {
    const res = send({
      type: "search",
      id: 1,
      query,
      allFilterPill: "all",
      aFilter: "all",
      dFilter: "all",
      eFilter: "all",
      searchCase: false,
      searchWord: false,
      searchRegex: false,
      searchContent: false,
      searchExcludeNonDefault: false,
      contentContextFilters: filters,
      contentMetadataFilters: [],
    })
    return Array.from(res.filteredAllIdx, (i) => keys[i]).sort()
  }
}

// ── The renderer half ────────────────────────────────────────────────────────
const ctx = vm.createContext({ console })
vm.runInContext(
  `
  let allArticles = []
  let allDialogsCombined = []
  let contentContextOptions = []
  let contentContextFilters = []
  let ctxVarMap = new Map()
  let dialogMap = new Map()
  let tDialogMap = new Map()
  ${[
    "_outputCtxSet",
    "_itemOutputCtxSets",
    "buildContentContextOptions",
    "_tagFilterMatches",
    "_routeTargetLabel",
    "_itemRouteOutputs",
    "contextRouteMatchSnippet",
  ]
    .map(extract)
    .join("\n")}
  function setData(articles, dialogs, ctxVars) {
    allArticles = articles
    allDialogsCombined = dialogs
    ctxVarMap = new Map(ctxVars.map((v) => [v.id, v.name]))
    dialogMap = new Map(dialogs.map((d) => [d.id, d]))
    buildContentContextOptions()
    return contentContextOptions
  }
  function setFilters(f) { contentContextFilters = f }
`,
  ctx,
)

// ── Fixture ──────────────────────────────────────────────────────────────────
const THEATER = 28
const ESC = 5
const CHANNEL = 9
const CTX_VARS = [
  { id: THEATER, name: "available_livechat_theater" },
  { id: ESC, name: "escalationGroup" },
  { id: CHANNEL, name: "Channel" },
]
// Article-shape variables: the export lists every variable on every output,
// "any" where the output does not care.
const acv = (theater, extra = []) => [
  { Id: THEATER, Values: [theater] },
  { Id: CHANNEL, Values: ["any"] },
  ...extra,
]
const route = (dialogId, isDefault, theater) => ({
  Type: "DialogStart",
  DialogId: dialogId,
  IsDefault: isDefault,
  OutputMetaData: {},
  ContextVariables: acv(theater),
})
const answer = (isDefault, cvs, text = "t") => ({
  Type: "Answer",
  Text: text,
  IsDefault: isDefault,
  OutputMetaData: {},
  ContextVariables: cvs,
})
const article = (Id, Outputs) => ({
  Id,
  Culture: "nl",
  Questions: [{ Text: "q" + Id, IsFaq: true }],
  Outputs,
  Categories: [],
})

// Article 737's shape: its default output routes into a Dialog, and which
// Dialog it routes into depends on whether the theater live chat is open.
const A737 = article(737, [
  route(900, true, "any"),
  route(901, false, "true"),
  route(902, false, "outside_schedule"),
])
const A_ANSWER = article(10, [
  answer(true, acv("any")),
  answer(false, acv("true"), "theater is open"),
])
const A_PLAIN = article(11, [answer(true, acv("any"))])
// Conditioned only on the escalationGroup *variable*, which the Context tab
// filters through the metadata tag instead — so for every other key it is an
// output that sets nothing.
const A_ESC_COND = article(12, [
  route(900, true, "any"),
  answer(false, acv("any", [{ Id: ESC, Values: ["theater"] }])),
])

const item = (type, cvs, data = {}) => ({
  type,
  isDefault: false,
  metadata: {},
  data,
  contextVariables: cvs,
})
const dialog = (id, name, items) => ({
  id,
  name,
  nodes: [
    { id: id * 10, type: "Output", name: "n" + id, output: { items }, links: [] },
  ],
})
const D_ROUTE = dialog(900, "Routes by theater", [
  item("DialogStart", [{ id: THEATER, value: "true" }], { dialogId: 901 }),
  item("TDialogStart", [], { tDialogId: 5 }),
])
// Every output conditioned — no output leaves the key unset.
const D_ROUTE_ONLY = dialog(901, "Theater only", [
  item("DialogStart", [{ id: THEATER, value: "true,false" }], { dialogId: 902 }),
])
const D_PLAIN = dialog(902, "Plain", [item("Answer", [], { text: "hi" })])

const ARTS = [A737, A_ANSWER, A_PLAIN, A_ESC_COND]
const DLGS = [D_ROUTE, D_ROUTE_ONLY, D_PLAIN]
const search = searcher(ARTS, DLGS, CTX_VARS)
const f = (value) => [{ name: "available_livechat_theater", value }]

// ── What the filter returns ──────────────────────────────────────────────────
eq(
  "a condition on a route is found, not only one on an Answer",
  search(f("__any__")),
  ["a10", "a737", "d900", "d901"],
)
eq(
  "a value set only on a route returns that item",
  search(f("outside_schedule")),
  ["a737"],
)
eq(
  "a dialog's comma-separated route value splits into values",
  search(f("false")),
  ["d901"],
)
eq(
  "an unconditioned route counts as an output that leaves the key unset",
  search(f("__not_set__")),
  ["a10", "a11", "a12", "a737", "d900", "d902"],
)
eq(
  "an escalationGroup variable is not a condition on another key",
  search([{ name: "Channel", value: "__not_set__" }]).includes("a12"),
  true,
)
// Two keys must be satisfied by one output, exactly as before.
eq(
  "filters on two keys still need one output to satisfy both",
  search([
    { name: "available_livechat_theater", value: "true" },
    { name: "Channel", value: "__any__" },
  ]),
  [],
)
// With a text query the condition has to sit on the Answer the text matched —
// a route has no text, so it cannot be the match.
eq(
  "with a query, only the matching Answer's condition counts",
  search(f("true"), "open"),
  ["a10"],
)

// ── The chip counts are what the filter returns ──────────────────────────────
function checkCounts(label, articles, dialogs, ctxVars) {
  const options = ctx.setData(articles, dialogs, ctxVars)
  const run = searcher(articles, dialogs, ctxVars)
  const wrong = []
  for (const o of options) {
    const got = run([{ name: o.name, value: o.value }]).length
    if (got !== o.count)
      wrong.push(`${o.name}=${o.value}: chip ${o.count}, filter ${got}`)
  }
  ok(
    `${label}: every chip's count is what the filter returns (${options.length} chips)`,
    wrong.length === 0,
  )
  if (wrong.length) out.push("         " + wrong.slice(0, 5).join("; "))
  return options
}

const fixtureChips = checkCounts("fixture", ARTS, DLGS, CTX_VARS)
const chip = (name, value) =>
  fixtureChips.find((o) => o.name === name && o.value === value)
eq(
  "a chip counts items, not outputs",
  chip("available_livechat_theater", "true").count,
  4,
)
ok(
  "a value only a route sets still gets a chip",
  !!chip("available_livechat_theater", "outside_schedule"),
)

// ── The card says why it matched ─────────────────────────────────────────────
ctx.setData(ARTS, DLGS, CTX_VARS)
const snippet = (it, filters, q = "") => {
  ctx.setFilters(filters)
  return ctx.contextRouteMatchSnippet(it, q)
}
eq(
  "a route match explains itself on the card",
  snippet(A737, f("outside_schedule")),
  {
    label: "Context route",
    text: "Dialog: Plain · available_livechat_theater = outside_schedule",
  },
)
eq(
  "a dialog's route snippet names its node",
  snippet(D_ROUTE, f("true")).text,
  "n900: Dialog: Theater only · available_livechat_theater = true",
)
eq("no route snippet when an Answer matched", snippet(A_ANSWER, f("true")), null)
eq("no route snippet with a query", snippet(A737, f("true"), "x"), null)
eq("no route snippet without a filter", snippet(A737, []), null)

// ── Renderer and worker read an output identically ──────────────────────────
function checkMirror(label, articles, dialogs, ctxVars) {
  const { wctx } = loadWorker()
  wctx.__setCtxVars(ctxVars)
  ctx.setData([], [], ctxVars)
  let n = 0
  const diffs = []
  const cmp = (cvs, isArticle) => {
    n++
    const w = JSON.stringify(wctx.__outputCtxSet(cvs, isArticle))
    const r = JSON.stringify(ctx._outputCtxSet(cvs, isArticle))
    if (w !== r) diffs.push(r + " vs " + w)
  }
  for (const a of articles) for (const o of a.Outputs) cmp(o.ContextVariables, true)
  for (const d of dialogs)
    for (const node of d.nodes || [])
      for (const oi of (node.output && node.output.items) || [])
        cmp(oi.contextVariables, false)
  ok(`${label}: renderer and worker agree on every output's context (${n})`, !diffs.length)
  if (diffs.length) out.push("         " + diffs.slice(0, 3).join("; "))
}
checkMirror("fixture", ARTS, DLGS, CTX_VARS)

// ── The real export ──────────────────────────────────────────────────────────
// Checked out beside the app, or pointed at with CAI_EXPORT_DIR.
const exportDir = process.env.CAI_EXPORT_DIR || ROOT
const pick = (pattern) =>
  fs
    .readdirSync(exportDir)
    .filter((n) => n.includes(pattern) && n.endsWith(".json"))
    .sort()
    .pop()
const artPath = pick("ArticlesExport")
const dlgPath = pick("DialogsExport")
if (artPath && dlgPath) {
  const A = JSON.parse(fs.readFileSync(path.join(exportDir, artPath), "utf8"))
  const D = JSON.parse(fs.readFileSync(path.join(exportDir, dlgPath), "utf8"))
  const articles = A.Articles || A.articles || []
  const dialogs = (D.dialogs && D.dialogs.result) || D.dialogs || []
  const ctxVars = D.contextVariables || []
  const real = checkCounts("real export", articles, dialogs, ctxVars)
  checkMirror("real export", articles, dialogs, ctxVars)

  // Would an Answer-only reading have found fewer? If routes never add an
  // item the export has changed shape, and this file is no longer testing
  // what it was written for.
  const answerOnly = new Map()
  const bump = (name, key) =>
    (answerOnly.get(name) || answerOnly.set(name, new Set()).get(name)).add(key)
  for (const a of articles)
    for (const o of a.Outputs)
      if (o.Type === "Answer")
        for (const name of Object.keys(ctx._outputCtxSet(o.ContextVariables, true)))
          bump(name, "a" + a.Id)
  for (const d of dialogs)
    for (const node of d.nodes || [])
      for (const oi of (node.output && node.output.items) || [])
        if (oi.type === "Answer")
          for (const name of Object.keys(ctx._outputCtxSet(oi.contextVariables, false)))
            bump(name, "d" + d.id)
  const gained = real.filter(
    (o) =>
      o.value === "__any__" &&
      o.count > ((answerOnly.get(o.name) && answerOnly.get(o.name).size) || 0),
  )
  ok(
    `real export: routes add items to ${gained.length} context keys (${artPath})`,
    gained.length > 0,
  )
} else {
  out.push("  SKIP  real export not present (set CAI_EXPORT_DIR)")
}

console.log(out.join("\n"))
if (failed) {
  console.error(`\nContext filter: ${failed} check(s) failed`)
  process.exit(1)
}
console.log("\nContext filter: all checks passed")

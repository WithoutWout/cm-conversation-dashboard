// Content-side metadata filtering: the `OutputMetaData` tags an Answer carries,
// filtered from the Metadata tab of the tag popover.
//
// Two halves have to agree or the UI lies about itself: `buildContentMetadataOptions`
// (in index.html) decides which chips exist and what number each one shows, and
// `matchesContentMetadata` (in search-worker.js) decides which items a chip
// actually returns. A chip saying "9 items" that filters down to 4 is worse
// than no chip at all, so the last section here asserts every chip's count
// against the set the matcher produces — over the real export when it is
// present, and over a fixture otherwise.
const { extract } = require("./extract")
const fs = require("fs")
const path = require("path")
const vm = require("vm")

const ROOT = path.join(__dirname, "..", "..")

// ── The renderer half ────────────────────────────────────────────────────────
const RENDERER = [
  "_unmarkJsonBreaks",
  "_flattenMetaEntry",
  "_metaNamesOf",
  "buildContentMetadataOptions",
  "_contentMetaPartialCount",
]
const ctx = vm.createContext({ console })
vm.runInContext(
  `
  let allArticles = []
  let allDialogsCombined = []
  let contentMetadataOptions = []
  const META_MAX_DEPTH = 3
  const META_MAX_LEAVES = 24
  ${RENDERER.map(extract).join("\n")}
  function setData(articles, dialogs) {
    allArticles = articles
    allDialogsCombined = dialogs
    buildContentMetadataOptions()
    return contentMetadataOptions
  }
`,
  ctx,
)

// ── The worker half ──────────────────────────────────────────────────────────
// Pulled out of search-worker.js the same way, so the matcher under test is the
// one that actually runs.
const workerSrc = fs.readFileSync(
  path.join(__dirname, "..", "search-worker.js"),
  "utf8",
)
function extractFrom(src, name) {
  const at = src.indexOf("function " + name + "(")
  if (at === -1) throw new Error("not found in worker: " + name)
  const start = src.indexOf("{", src.indexOf(")", at))
  let depth = 0
  for (let j = start; j < src.length; j++) {
    if (src[j] === "{") depth++
    else if (src[j] === "}" && --depth === 0) return src.slice(at, j + 1)
  }
  throw new Error("unbalanced: " + name)
}
const wctx = vm.createContext({ console })
vm.runInContext(
  `
  let contentMetadataFilters = []
  const META_MAX_DEPTH = 3
  const META_MAX_LEAVES = 24
  const EMPTY_TAG_SET = {}
  ${[
    "_unmarkJsonBreaks",
    "flattenMetaEntry",
    "metaSetOf",
    "tagFilterMatches",
    "matchesContentMetadata",
  ]
    .map((n) => extractFrom(workerSrc, n))
    .join("\n")}
  function setFilters(f) { contentMetadataFilters = f }
  // The two precompute passes reduced to what this matcher reads. Kept here
  // rather than extracted because precomputeArticle/precomputeDialog also build
  // the search, context and entity fields, none of which this is about.
  function metaSets(item) {
    const sets = []
    if (item.Outputs) {
      for (const o of item.Outputs) if (o.Type === "Answer") sets.push(metaSetOf(o.OutputMetaData))
    } else {
      for (const n of item.nodes || [])
        for (const oi of ((n.output && n.output.items) || []))
          if (oi.type === "Answer") sets.push(metaSetOf(oi.metadata))
    }
    return sets
  }
  function matches(item) { return matchesContentMetadata({ _metaSets: metaSets(item) }) }
`,
  wctx,
)

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

// ── metaSetOf: the export's shapes ───────────────────────────────────────────
const metaSetOf = (m) => wctx.metaSetOf(m)
eq("reads a plain string value", metaSetOf({ nochat: "false" }), {
  nochat: ["false"],
})
// The export mixes "true" with true; a chip the user clicks is a string either
// way, so both must land on the same value.
eq("normalises a boolean to its string", metaSetOf({ nochat: true }), {
  nochat: ["true"],
})
eq("normalises a number to its string", metaSetOf({ entryLimit: 10 }), {
  entryLimit: ["10"],
})
// "set but empty" is a real, distinct state and must not collapse into absent.
eq("keeps an empty value as an empty string", metaSetOf({ showName: "" }), {
  showName: [""],
})
eq("treats null as empty", metaSetOf({ showName: null }), { showName: [""] })
eq("trims the key", metaSetOf({ "  entryType  ": "choice_prompt" }), {
  entryType: ["choice_prompt"],
})
eq("drops a whitespace-only key", metaSetOf({ "   ": "x" }), {})
for (const junk of [null, undefined, "", 0, "a string"]) {
  ok(`${JSON.stringify(junk)} yields no pairs`, Object.keys(metaSetOf(junk)).length === 0)
}

// ── Nested values ────────────────────────────────────────────────────────────
// A JSON-object value rendered whole is an unreadable blob of a chip, and the
// same logical value arrives in three spellings that would each be their own
// chip. Splitting it into `key.subkey` fixes both at once.
const ABORT = {
  compact: `{"label":"Ik wil geen medewerker spreken","topicName":"Stoppen_TD_Algemeen"}`,
  // CM replaces newlines in an authored value with its `_` line-break marker.
  broken: `{__  "label": "Ik wil geen medewerker spreken",__  "topicName": "Stoppen_TD_Algemeen"__}`,
  swapped: `{__  "topicName": "Stoppen_TD_Algemeen",__  "label": "Ik wil geen medewerker spreken"__}`,
}
const EXPANDED = {
  "abortTransactionAction.label": ["Ik wil geen medewerker spreken"],
  "abortTransactionAction.topicName": ["Stoppen_TD_Algemeen"],
}
// Key order is not compared: the popover sorts groups by name before
// rendering, so the order `metaSetOf` happens to build them in is invisible —
// and the whole point of this case is that a value whose keys arrive swapped
// must land on the same set.
const canonical = (o) =>
  Object.keys(o)
    .sort()
    .map((k) => `${k}=${o[k].join("|")}`)
for (const [name, raw] of Object.entries(ABORT)) {
  eq(
    `a nested value is split into its leaves (${name} spelling)`,
    canonical(metaSetOf({ abortTransactionAction: raw })),
    canonical(EXPANDED),
  )
}
// The underscore inside a value is part of the value, not a marker.
ok(
  "an underscore inside a JSON string survives",
  metaSetOf({ abortTransactionAction: ABORT.broken })[
    "abortTransactionAction.topicName"
  ][0] === "Stoppen_TD_Algemeen",
)
eq(
  "_unmarkJsonBreaks never edits inside a quoted span",
  ctx._unmarkJsonBreaks(`{__"a": "x_y"__}`),
  `{ "a": "x_y" }`,
)
eq(
  "an escaped quote does not end the string early",
  ctx._unmarkJsonBreaks(`{"a": "say \\"x_y\\" now"}`),
  `{"a": "say \\"x_y\\" now"}`,
)
// An already-parsed object (the content export's own shape) takes the same path.
eq(
  "an object value is split without needing to parse",
  metaSetOf({ act: { label: "Stoppen", topicName: "T" } }),
  { "act.label": ["Stoppen"], "act.topicName": ["T"] },
)
// Only objects. An array has no stable member names, so splitting it would
// invent filter keys that mean nothing.
eq("an array value stays one pair", metaSetOf({ k: "[1,2,3]" }), { k: ["[1,2,3]"] })
eq("unparseable braces stay one pair", metaSetOf({ k: "{not json}" }), {
  k: ["{not json}"],
})
eq("an empty object keeps its key", metaSetOf({ k: "{}" }), { k: [""] })
// Bounded, so a large embedded document can't become hundreds of chips.
const deep = { a: { b: { c: { d: { e: "too deep" } } } } }
ok(
  "nesting stops at META_MAX_DEPTH",
  Object.keys(metaSetOf(deep)).every((n) => n.split(".").length <= 4),
)
const wide = {}
for (let i = 0; i < 100; i++) wide["k" + i] = "v"
ok(
  "one value contributes at most META_MAX_LEAVES",
  Object.keys(metaSetOf({ big: JSON.stringify(wide) })).length <= 24,
)

// The renderer and the worker each carry their own copy of the flattener —
// there is no module boundary to share one across. They must agree exactly.
const flattenPairs = (key, value) => {
  const r = ctx._flattenMetaEntry(key, value, [], 0)
  const w = wctx.flattenMetaEntry(key, value, [], 0)
  return [JSON.stringify(r), JSON.stringify(w)]
}
const flattenCases = [
  ["abortTransactionAction", ABORT.compact],
  ["abortTransactionAction", ABORT.broken],
  ["abortTransactionAction", ABORT.swapped],
  ["k", "[1,2,3]"],
  ["k", "{}"],
  ["k", "{not json}"],
  ["k", ""],
  ["k", null],
  ["k", true],
  ["k", 10],
  ["  padded  ", "v"],
  ["act", { label: "Stoppen", nested: { deep: "x" } }],
]
const flattenDrift = flattenCases.filter(([k, v]) => {
  const [r, w] = flattenPairs(k, v)
  return r !== w
})
eq(
  "the renderer and worker flatteners agree on every shape",
  flattenDrift.map(([k, v]) => `${k}=${JSON.stringify(v)}`),
  [],
)

// ── matchesContentMetadata ───────────────────────────────────────────────────
const article = (Id, outputs) => ({
  Id,
  Questions: [{ Text: "q" + Id, IsFaq: true }],
  Outputs: outputs,
})
const answer = (meta, isDefault) => ({
  Type: "Answer",
  Text: "text",
  IsDefault: !!isDefault,
  OutputMetaData: meta,
})

const A_BOTH = article(1, [
  answer({ nochat: "true", entryType: "choice_prompt" }, true),
])
const A_SPLIT = article(2, [
  answer({ nochat: "true" }, true),
  answer({ entryType: "choice_prompt" }, false),
])
const A_NONE = article(3, [answer(undefined, true)])
const A_ROUTING = article(4, [
  { Type: "DialogStart", DialogId: 9, IsDefault: true },
])

const m = (item, filters) => {
  wctx.setFilters(filters)
  return wctx.matches(item)
}

ok("no filters matches everything", m(A_NONE, []) && m(A_BOTH, []))
ok("a value filter matches the item carrying it", m(A_BOTH, [{ name: "nochat", value: "true" }]))
ok(
  "a value filter rejects an item without it",
  !m(A_NONE, [{ name: "nochat", value: "true" }]),
)
ok(
  "the value must match, not just the key",
  !m(A_BOTH, [{ name: "nochat", value: "false" }]),
)

// Two filters on different keys must be satisfied by ONE answer, matching how
// context filters behave. A_SPLIT sets each on a different answer, so it is not
// a match even though the item as a whole carries both.
ok(
  "two keys must be satisfied by the same answer",
  m(A_BOTH, [
    { name: "nochat", value: "true" },
    { name: "entryType", value: "choice_prompt" },
  ]) &&
    !m(A_SPLIT, [
      { name: "nochat", value: "true" },
      { name: "entryType", value: "choice_prompt" },
    ]),
)

// "not set" per key. An answer with no metadata at all is a real entry in
// _metaSets — an empty map — so it satisfies "not set" without a special case.
ok(
  "not set matches an item that never sets the key",
  m(A_NONE, [{ name: "nochat", value: "__not_set__" }]),
)
ok(
  "not set rejects an item that always sets it",
  !m(A_BOTH, [{ name: "nochat", value: "__not_set__" }]),
)
ok(
  "not set matches an item setting it on only some answers",
  m(A_SPLIT, [{ name: "nochat", value: "__not_set__" }]),
)
// An item with no Answer outputs at all (an Article that routes into a Dialog)
// has nothing to satisfy a value filter, but "not set" is trivially true of it.
ok(
  "a routing-only item satisfies not set and nothing else",
  m(A_ROUTING, [{ name: "nochat", value: "__not_set__" }]) &&
    !m(A_ROUTING, [{ name: "nochat", value: "true" }]),
)

// ── buildContentMetadataOptions ──────────────────────────────────────────────
const dialog = (id, nodeMetas) => ({
  id,
  name: "d" + id,
  _kind: "dialog",
  nodes: [
    {
      id: 1,
      type: "Output",
      name: "n1",
      output: {
        items: nodeMetas.map((meta) => ({
          type: "Answer",
          isDefault: true,
          data: { text: "t" },
          metadata: meta,
        })),
      },
      links: [],
    },
  ],
})

const opts = ctx.setData(
  [A_BOTH, A_SPLIT, A_NONE, A_ROUTING],
  [dialog(50, [{ nochat: "false" }])],
)
const chip = (name, value) =>
  opts.find((o) => o.name === name && o.value === value)

ok("a chip exists for each distinct key/value", !!chip("nochat", "true"))
eq("chips count items, not answers", chip("nochat", "true").count, 2)
ok("a dialog's metadata gets its own chip", !!chip("nochat", "false"))
eq("that dialog chip counts one item", chip("nochat", "false").count, 1)

// escalationGroup already has chips on the Context tab, where it belongs — it
// is a declared context variable with its own Id. Listing it here too would let
// a user set two filters that look independent and describe the same thing.
const withEsc = ctx.setData(
  [article(9, [answer({ escalationGroup: "attractiepark", nochat: "true" }, true)])],
  [],
)
ok(
  "escalationGroup is never offered as a metadata chip",
  !withEsc.some((o) => o.name === "escalationGroup"),
)
ok("other keys on the same answer still are", withEsc.some((o) => o.name === "nochat"))

// ── The counts must equal what the filter returns ────────────────────────────
// This is the assertion that actually protects the user: a chip labelled
// "N items" must return exactly N. Run over the real export when it is checked
// out beside the app, so the shapes are the ones the tool really sees.
function checkCounts(label, articles, dialogs) {
  const options = ctx.setData(articles, dialogs)
  const items = [...articles, ...dialogs]
  const wrong = []
  for (const o of options) {
    const got = items.filter((it) => m(it, [{ name: o.name, value: o.value }]))
      .length
    if (got !== o.count) wrong.push(`${o.name}=${o.value || "(empty)"}: chip ${o.count}, filter ${got}`)
  }
  ok(
    `${label}: every chip's count is what the filter returns (${options.length} chips)`,
    wrong.length === 0,
  )
  if (wrong.length) out.push("         " + wrong.slice(0, 5).join("; "))
  return options
}

checkCounts("fixture", [A_BOTH, A_SPLIT, A_NONE, A_ROUTING], [dialog(50, [{ nochat: "false" }])])

const artPath = fs
  .readdirSync(ROOT)
  .find((f) => f.includes("ArticlesExport") && f.endsWith(".json"))
const dlgPath = fs
  .readdirSync(ROOT)
  .find((f) => f.includes("DialogsExport") && f.endsWith(".json"))
if (artPath && dlgPath) {
  const A = JSON.parse(fs.readFileSync(path.join(ROOT, artPath), "utf8"))
  const D = JSON.parse(fs.readFileSync(path.join(ROOT, dlgPath), "utf8"))
  const articles = A.Articles || A.articles || []
  const dialogs = (D.dialogs && D.dialogs.result) || D.dialogs || []
  const real = checkCounts("real export", articles, dialogs)
  ok(
    `real export yields metadata chips (${real.length} across ${articles.length} Articles + ${dialogs.length} Dialogs)`,
    real.length > 0,
  )
  // The keys the export actually carries — if this drops to a handful, the
  // shape has changed and the reader above is silently missing most of it.
  const names = new Set(real.map((o) => o.name))
  ok(`real export carries many distinct metadata keys (${names.size})`, names.size >= 10)
} else {
  out.push("  SKIP  real export not present beside the app")
}

console.log(out.join("\n"))
if (failed) {
  console.error(`\nMetadata filter: ${failed} check(s) failed`)
  process.exit(1)
}
console.log("\nMetadata filter: all checks passed")

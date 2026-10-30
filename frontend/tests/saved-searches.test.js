// Saved searches: `cm-saved-searches` is read entry by entry, so a hand-edited
// or older file cannot bring in a view, a chip type or a pill this build does
// not know, and two saves of one search are recognised as the same search.
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = vm.createContext({})
vm.runInContext(
  `
  const SAVED_SEARCH_VIEWS = ["content", "conversations"]
  const SAVED_EXPR_TYPES = new Set(["text", "id", "entity", "tag", "op", "(", ")"])
  const CONV_FILTERS = ["all", "genai", "any_feedback", "pos_feedback", "neg_feedback", "low_recog", "zero_recog"]
  const localStorage = { getItem: () => null }
  const document = { querySelector: () => null }
  function exprToQuery(list) { return list.filter((i) => i.t === "text").map((i) => i.value).join(" AND ") }
  ${["loadSavedSearches", "_savedCleanTags", "_savedCleanState", "savedSearchIsEmpty", "_savedKey", "savedSearchSummary"]
    .map(extract)
    .join("\n")}
  globalThis.api = { loadSavedSearches, savedSearchIsEmpty, _savedKey, savedSearchSummary, _savedCleanState }
  `,
  ctx,
)
const api = ctx.api
const plain = (v) => JSON.parse(JSON.stringify(v))

let failed = 0
const test = (name, fn) => {
  try {
    fn()
    console.log("  PASS  " + name)
  } catch (e) {
    failed++
    console.log("  FAIL  " + name + "\n        " + e.message)
  }
}

test("a stored list is read entry by entry", () => {
  const raw = JSON.stringify([
    { id: "a", view: "content", name: "Korting", state: { expr: [{ t: "text", value: "korting" }, { t: "evil" }], case: 1 } },
    { id: "b", view: "insights", name: "Unknown view", state: { expr: [] } },
    { id: "c", view: "conversations", name: "  ", state: { expr: [] } },
    { id: "d", view: "conversations", name: "Boos", state: { expr: [], filter: "drop_tables", scope: "sideways", context: [{ name: "x", value: 1 }] } },
    "nonsense",
  ])
  const got = plain(api.loadSavedSearches(raw))
  assert.deepStrictEqual(got.map((e) => e.id), ["a", "d"])
  assert.deepStrictEqual(got[0].state.expr, [{ t: "text", value: "korting" }], "an unknown chip type is dropped")
  assert.strictEqual(got[0].state.case, true)
  assert.strictEqual(got[1].state.filter, "all", "an unknown pill falls back to none")
  assert.strictEqual(got[1].state.scope, "user")
  assert.deepStrictEqual(got[1].state.context, [], "a tag filter must be two strings")
  assert.deepStrictEqual(plain(api.loadSavedSearches("{not json")), [])
})

test("an empty bar is not a search; a pill or a tag filter alone is", () => {
  const conv = (st) => api._savedCleanState("conversations", { expr: [], ...st })
  assert.strictEqual(api.savedSearchIsEmpty(conv({})), true)
  assert.strictEqual(api.savedSearchIsEmpty(conv({ filter: "neg_feedback" })), false)
  assert.strictEqual(api.savedSearchIsEmpty(conv({ context: [{ name: "lang", value: "nl" }] })), false)
})

test("a chip's transient fields do not make one search two", () => {
  const a = api._savedCleanState("content", { expr: [{ t: "id", idText: "qa-1", kind: "article" }] })
  const b = api._savedCleanState("content", { expr: [{ t: "id", idText: "qa-1", kind: "article", label: "read later" }] })
  assert.strictEqual(api._savedKey(a), api._savedKey(b))
})

test("the summary writes the search as the bar would", () => {
  const st = api._savedCleanState("conversations", {
    expr: [{ t: "text", value: "boos" }],
    regex: true,
    scope: "both",
    metadata: [{ name: "a", value: "b" }],
  })
  assert.strictEqual(api.savedSearchSummary("conversations", st), "boos · .* U+B · 1 tag filter")
})

if (failed) {
  console.error("Saved searches: " + failed + " check(s) failed")
  process.exit(1)
}
console.log("Saved searches: all checks passed")

// The Article and Dialog info modals show each output's context conditions and
// metadata tags — default Responses, contextual ones and routes alike — through
// `renderOutputTagsHtml`, run here from the real source.
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = vm.createContext({ console })
vm.runInContext(
  `
  let contentExpr = []
  let contentContextFilters = []
  let contentMetadataFilters = []
  const ctxVarMap = new Map([[1, "klant"], [2, "DeviceType"], [3, "escalationGroup"]])
  let HIDDEN = []
  function metaHidden(name, value) {
    return HIDDEN.some((h) => h === name || (value != null && h === name + " = " + value))
  }
  function _unmarkJsonBreaks(t) { return t }
  const META_MAX_DEPTH = 3
  const META_MAX_LEAVES = 24
  // Not extracted: an earlier \`function esc(\` in index.html is found first.
  function esc(s) {
    return String(s == null ? "" : s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;")
  }
  ${["_ctxAdd", "_outputEscGroups", "_flattenMetaEntry", "_activeTagConds", "_tagIsSearched", "renderOutputTagsHtml"].map(extract).join("\n")}
  globalThis.T = {
    tags: renderOutputTagsHtml,
    set(o) {
      contentExpr = o.expr || []
      contentContextFilters = o.ctx || []
      contentMetadataFilters = o.meta || []
      HIDDEN = o.hidden || []
    },
  }
  `,
  ctx,
)
const T = ctx.T
const labels = (html) => [...html.matchAll(/<span class="([^"]+)"[^>]*>([^<]*)<\/span>/g)].map((m) => [m[1], m[2]])

let failed = 0
function test(name, fn) {
  try {
    T.set({})
    fn()
    console.log("  PASS  " + name)
  } catch (e) {
    failed++
    console.log("  FAIL  " + name + "\n        " + e.message)
  }
}

const DEVICE = [{ id: 2, value: "Tablet" }, { id: 2, value: "Mobile" }, { id: 2, value: "any" }]

test("a Dialog's repeated condition is one pill with every value, `any` left out", () => {
  assert.deepStrictEqual(labels(T.tags(DEVICE, null, false)), [["ctx-var-pill", "DeviceType: Tablet, Mobile"]])
})

test("an Article condition reads its Values array", () => {
  assert.deepStrictEqual(labels(T.tags([{ Id: 1, Values: ["leden", "any"] }], null, true)), [["ctx-var-pill", "klant: leden"]])
})

test("escalationGroup by tag and by condition is one pill, on the context side", () => {
  const html = T.tags([{ id: 3, value: "theater" }], { escalationGroup: "attractiepark" }, false)
  assert.deepStrictEqual(labels(html), [["ctx-var-pill", "escalationGroup: attractiepark, theater"]])
})

test("metadata is shown as key = value, JSON flattened as the filter reads it", () => {
  const html = T.tags([], { nochat: "true", abortTransactionAction: '{"label":"Stoppen","topicName":"X"}' }, false)
  assert.deepStrictEqual(labels(html), [
    ["ctx-var-pill meta-pill", "nochat = true"],
    ["ctx-var-pill meta-pill", "abortTransactionAction.label = Stoppen"],
    ["ctx-var-pill meta-pill", "abortTransactionAction.topicName = X"],
  ])
})

test("metadata hidden in Settings stays hidden, by key or by value", () => {
  T.set({ hidden: ["hideFeedback", "entryType = choice"] })
  const html = T.tags([], { hideFeedback: "true", entryType: "choice", nochat: "true" }, false)
  assert.deepStrictEqual(labels(html).map((l) => l[1]), ["nochat = true"])
})

test("a pill the search asks for is marked — chip or panel, context or metadata", () => {
  T.set({ expr: [{ t: "tag", kind: "context", name: "DeviceType", values: ["Mobile"] }], meta: [{ name: "nochat", value: "true" }] })
  const html = T.tags(DEVICE, { nochat: "true", hideFeedback: "true" }, false)
  assert.deepStrictEqual(labels(html), [
    ["ctx-var-pill ctx-var-pill-match", "DeviceType: Tablet, Mobile"],
    ["ctx-var-pill meta-pill ctx-var-pill-match", "nochat = true"],
    ["ctx-var-pill meta-pill", "hideFeedback = true"],
  ])
})

test("a negated chip marks nothing", () => {
  T.set({ expr: [{ t: "op", op: "not" }, { t: "tag", kind: "metadata", name: "nochat", values: [] }] })
  assert.ok(!T.tags([], { nochat: "true" }, false).includes("ctx-var-pill-match"))
})

test("an output with neither draws nothing", () => {
  assert.strictEqual(T.tags([], {}, false), "")
  assert.strictEqual(T.tags([{ id: 2, value: "any" }], null, false), "")
})

console.log(failed ? "\n" + failed + " failing" : "Modal tags: all checks passed")
process.exit(failed ? 1 : 0)

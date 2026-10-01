// Grouping chips already in the field: select a run, wrap it in brackets,
// or remove it — `exprSelectionSpan`, `exprWrapSpan`, `exprRemoveSpan`, run
// from the real source with the real `exprNormalize` and `exprToQuery`.
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = vm.createContext({ console })
vm.runInContext(
  `
  function _convIsOperand(it) { return it && (it.t === "text" || it.t === "id" || it.t === "entity" || it.t === "tag") }
  function _convEscapeText(v) { return v }
  function _convEntityToken(v) { return "entity:" + v }
  function _exprTagToken(it) { return "ctx:" + it.name }
  ${["_exprPartner", "exprSelectionSpan", "exprNormalize", "exprWrapSpan", "exprRemoveSpan", "exprMoveSpan", "exprToQuery"].map(extract).join("\n")}
  globalThis.T = { move: exprMoveSpan, span: exprSelectionSpan, wrap: exprWrapSpan, remove: exprRemoveSpan, q: exprToQuery, norm: exprNormalize }
  `,
  ctx,
)
const T = ctx.T
const t = (value) => ({ t: "text", value })
const op = (o) => ({ t: "op", op: o })
const O = { t: "(" }
const C = { t: ")" }
// a and b or c and d
const FLAT = T.norm([t("a"), op("and"), t("b"), op("or"), t("c"), op("and"), t("d")])
// a and (b or c) and d
const GROUPED = T.norm([t("a"), op("and"), O, t("b"), op("or"), t("c"), C, op("and"), t("d")])

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
// `exprToQuery` writes "( b OR c )"; compared with the spaces folded.
const q = (list) => T.q(list).replace(/\( /g, "(").replace(/ \)/g, ")")
const same = (a, b) => assert.strictEqual(JSON.stringify(a), JSON.stringify(b))

test("wrapping two chips adds brackets and keeps the operators", () => {
  // select b (2) … c (4)
  const out = T.wrap(FLAT, T.span(FLAT, 2, 4))
  assert.strictEqual(q(out), "a AND (b OR c) AND d")
})

test("selecting either way round is the same span", () => {
  same(T.span(FLAT, 4, 2), T.span(FLAT, 2, 4))
})

test("a selection reaching into a group takes the whole group", () => {
  // a (0) … b (3, inside the group) → widened to the group's ")" (6)
  same(T.span(GROUPED, 0, 3), { a: 0, b: 6 })
  // c (5, inside) … d (8) → widened back to its "(" (2)
  same(T.span(GROUPED, 5, 8), { a: 2, b: 8 })
})

test("groups nest: wrapping around a group keeps it whole", () => {
  const out = T.wrap(GROUPED, T.span(GROUPED, 0, 5))
  assert.strictEqual(q(out), "(a AND (b OR c)) AND d")
})

test("a selection inside one group stays inside it", () => {
  same(T.span(GROUPED, 3, 3), { a: 3, b: 3 })
  assert.strictEqual(q(T.wrap(GROUPED, T.span(GROUPED, 3, 5))), "a AND ((b OR c)) AND d")
})

test("a leading not stays outside and applies to the group", () => {
  const list = T.norm([op("not"), t("a"), op("and"), t("b")])
  assert.strictEqual(q(T.wrap(list, T.span(list, 1, 3))), "NOT (a AND b)")
})

test("removing a run takes its operators with it", () => {
  assert.strictEqual(q(T.remove(FLAT, T.span(FLAT, 2, 4))), "a AND d")
  assert.strictEqual(q(T.remove(GROUPED, T.span(GROUPED, 3, 3))), "a AND (c) AND d")
})

test("removing a whole group's contents leaves no empty brackets", () => {
  assert.strictEqual(q(T.remove(GROUPED, T.span(GROUPED, 2, 6))), "a AND d")
})

// Move the chip whose text is \`word\` one step, and read the result.
const moved = (list, word, dir) => {
  const i = list.findIndex((it) => it.value === word)
  const r = T.move(list, T.span(list, i, i), dir)
  return r ? q(r.list) : null
}

test("Alt+→ trades places with the next chip; the operators stay put", () => {
  assert.strictEqual(moved(FLAT, "a", 1), "b AND a OR c AND d")
})

test("Alt+→ next to a group steps into it, at its start", () => {
  assert.strictEqual(moved(GROUPED, "a", 1), "(a AND b OR c) AND d")
})

test("Alt+← next to a group steps into it, at its end", () => {
  assert.strictEqual(moved(GROUPED, "d", -1), "a AND (b OR c AND d)")
})

test("the last chip of a group steps out of it, and the first one too", () => {
  assert.strictEqual(moved(GROUPED, "c", 1), "a AND (b) OR c AND d")
  assert.strictEqual(moved(GROUPED, "b", -1), "a AND b OR (c) AND d")
})

test("a whole group moves as one", () => {
  const i = GROUPED.findIndex((it) => it.t === "(")
  const r = T.move(GROUPED, T.span(GROUPED, i + 1, i + 1), 1) // b, inside
  assert.ok(r, "b moves within its group")
  const g = T.move(GROUPED, { a: 2, b: 6 }, 1)
  assert.strictEqual(q(g.list), "a AND d AND (b OR c)")
})

test("at either end there is nowhere to go", () => {
  assert.strictEqual(moved(FLAT, "d", 1), null)
  assert.strictEqual(moved(FLAT, "a", -1), null)
})

test("the moved chips can be found again after normalising", () => {
  const i = FLAT.findIndex((it) => it.value === "b")
  const r = T.move(FLAT, T.span(FLAT, i, i), 1)
  assert.strictEqual(r.first.value, "b")
  assert.ok(r.list.includes(r.first))
})

console.log(failed ? "\n" + failed + " failing" : "Expression grouping: all checks passed")
process.exit(failed ? 1 : 0)

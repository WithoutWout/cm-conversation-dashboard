// The Dialog modal reads as a flow: nodes in the order a conversation meets
// them from the entry points (`dialogFlow`), each way out in CM.com's
// evaluation order with fallbacks last (`_flowLinks`), and a Logical condition
// as words (`_flowLogicalText`). Run from the real source.
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = vm.createContext({ console })
vm.runInContext(
  `
  const convVarMap = new Map([[9, "klantType"]])
  const ctxVarMap = new Map()
  ${["_flowLinks", "dialogFlow", "_flowLogicalText"].map(extract).join("\n")}
  const _dialogFlowCache = new WeakMap()
  const _FLOW_OPS = { Equal: "is", NotEqual: "is not", Empty: "is empty", ContainsCaseInsensitive: "contains" }
  globalThis.T = { dialogFlow, _flowLinks, _flowLogicalText }
  `,
  ctx,
)
const T = ctx.T

const says = (child, prio, fallback) => ({
  childNodeId: child,
  condition: { type: "Recognition", data: { evaluationPriority: prio, isFallback: !!fallback, questions: [{ text: "x" }] } },
})
const goTo = (child) => ({ childNodeId: child, condition: { type: "GoTo", data: {} } })
const node = (id, name, links, type = "Output") => ({ id, name, type, links: links || [] })

// Export order is creation order: 40 (a later branch) comes before its parent.
const DIALOG = {
  id: 1,
  entryPoints: [{ nodeId: 10 }, { nodeId: 50 }],
  variables: [{ id: 1, name: "antwoord" }],
  nodes: [
    node(40, "Kosten", [goTo(10)], "GoTo"), // loops back to the start
    node(10, "Menu", [says(30, 2), says(99, 9, true), says(20, 1)], "Recognition"),
    node(20, "Tickets", [says(40, 1)]),
    node(30, "Hotel"),
    node(50, "Tweede start"),
    node(60, "Wees"), // nothing leads here
    node(99, "Anders"),
  ],
}

// Arrays built inside the vm context are not deepStrictEqual to this realm's.
const same = (a, b) => assert.strictEqual(JSON.stringify(a), JSON.stringify(b))

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

test("ways out are in evaluation order, fallbacks last", () => {
  const menu = DIALOG.nodes[1]
  same(T._flowLinks(menu).map((l) => l.childNodeId), [20, 30, 99])
})

test("nodes come depth-first from the entry points, a branch to its end first", () => {
  const f = T.dialogFlow(DIALOG)
  same(f.order.map((n) => n.id), [10, 20, 40, 30, 99, 50, 60])
})

test("a loop back to the start does not repeat a node", () => {
  const f = T.dialogFlow(DIALOG)
  assert.strictEqual(new Set(f.order.map((n) => n.id)).size, DIALOG.nodes.length)
})

test("steps are numbered in that order; nodes nothing reaches come last", () => {
  const f = T.dialogFlow(DIALOG)
  assert.strictEqual(f.step.get(10), 1)
  assert.strictEqual(f.step.get(40), 3)
  assert.strictEqual(f.step.get(60), 7)
})

test("each node knows what leads to it", () => {
  const f = T.dialogFlow(DIALOG)
  same(f.parents.get(10).map((n) => n.id), [40])
  same(f.parents.get(40).map((n) => n.id), [20])
  assert.strictEqual(f.parents.get(60), undefined)
  assert.ok(f.entries.has(50) && !f.entries.has(20))
})

test("without entry points the first node starts", () => {
  const f = T.dialogFlow({ id: 2, nodes: [node(5, "A", [says(6, 1)]), node(6, "B")] })
  same(f.order.map((n) => n.id), [5, 6])
})

test("a node sits under the parent closest to the start, not the first branch that reaches it", () => {
  // The menu lists A, B and C; A links to B and C again (a hub of options,
  // as "In het park" and its restaurants on Dialog 6270).
  const hub = {
    id: 3,
    entryPoints: [{ nodeId: 1 }],
    nodes: [
      node(1, "Menu", [says(2, 1), says(3, 2), says(4, 3)], "Recognition"),
      node(2, "A", [says(3, 1), says(4, 2)], "Recognition"),
      node(3, "B"),
      node(4, "C"),
    ],
  }
  const f = T.dialogFlow(hub)
  same(f.order.map((n) => n.id), [1, 2, 3, 4])
  same(f.tree.get(1).map((k) => [k.id, k.ref]), [[2, false], [3, false], [4, false]])
  same(f.tree.get(2).map((k) => [k.id, k.ref]), [[3, true], [4, true]])
})

test("a loop back is a reference, and every node is in the tree once", () => {
  const f = T.dialogFlow(DIALOG)
  same(f.tree.get(40).map((k) => [k.id, k.ref]), [[10, true]])
  const owned = [...f.tree.values()].flat().filter((k) => !k.ref).map((k) => k.id)
  same(owned.length + f.roots.filter((r) => !r.ref).length, DIALOG.nodes.length)
})

test("a Logical condition reads as words, groups as alternatives", () => {
  const data = {
    expressions: [
      [
        { leftOperand: { type: "DialogVariable", id: 1 }, operator: "Equal", rightOperand: { type: "Text", data: "Ja" } },
        { leftOperand: { type: "ConversationVariable", id: 9 }, operator: "Empty", rightOperand: null },
      ],
      [{ leftOperand: { type: "DialogVariable", id: 1 }, operator: "ContainsCaseInsensitive", rightOperand: { type: "Text", data: "nee" } }],
    ],
  }
  assert.strictEqual(
    T._flowLogicalText(data, DIALOG),
    "antwoord is “Ja” and klantType is empty or antwoord contains “nee”",
  )
})

console.log(failed ? "\n" + failed + " failing" : "Dialog flow: all checks passed")
process.exit(failed ? 1 : 0)

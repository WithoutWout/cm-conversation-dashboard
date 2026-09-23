// The GAP list's sort survives a restart (`cm-gap-sort`), and reading it back
// never trusts the stored value: a column that does not exist, a direction
// that is not ±1 or text that is not JSON all fall back to newest first.
const assert = require("assert")
const fs = require("fs")
const path = require("path")
const vm = require("vm")
const { extract } = require("./extract")

const src = fs.readFileSync(path.join(__dirname, "..", "index.html"), "utf8")
// The constants come from the page itself, so this cannot drift from it.
const constLine = (name) => {
  const m = new RegExp("const " + name + " = [^\\n]+").exec(src)
  if (!m) throw new Error("not found: " + name)
  return m[0]
}

const store = new Map()
const ctx = vm.createContext({
  localStorage: {
    getItem: (k) => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
  },
})
vm.runInContext(
  [constLine("GAP_SORT_STORAGE"), constLine("GAP_SORT_KEYS"), extract("gapLoadSort"), "globalThis.load = gapLoadSort; globalThis.KEY = GAP_SORT_STORAGE"].join("\n"),
  ctx,
)

const DEFAULT = { key: "time", dir: -1 }
const cases = [
  ["nothing stored is newest first", null, DEFAULT],
  ["a stored sort comes back", '{"key":"recognition","dir":1}', { key: "recognition", dir: 1 }],
  ["descending comes back too", '{"key":"question","dir":-1}', { key: "question", dir: -1 }],
  ["an unknown column is the default", '{"key":"nope","dir":1}', DEFAULT],
  ["a direction that is not ±1 is the default", '{"key":"response","dir":2}', DEFAULT],
  ["text that is not JSON is the default", "{broken", DEFAULT],
]
let failed = 0
for (const [name, stored, want] of cases) {
  store.clear()
  if (stored !== null) store.set(ctx.KEY, stored)
  try {
    assert.deepStrictEqual(JSON.parse(JSON.stringify(ctx.load())), want)
    console.log("  PASS  " + name)
  } catch (e) {
    failed++
    console.log("  FAIL  " + name + ": " + e.message)
  }
}
assert.strictEqual(ctx.KEY, "cm-gap-sort")
// …and it is carried by a settings backup.
assert.ok(/\{ key: "cm-gap-sort" \}/.test(src), "cm-gap-sort is not in SETTINGS_EXPORT_KEYS")
if (failed) {
  console.error("GAP sort: " + failed + " check(s) failed")
  process.exit(1)
}
console.log("GAP sort: all checks passed")

// The offer's list and its "ask again?" rule. Which files count as older is
// decided in Rust and tested there (`older_versions`).
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = {
  esc: (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;"),
  zoneDayKey: (ms) => new Date(ms).toISOString().slice(0, 10),
  insZone: () => "UTC",
  _fmtBytes: (n) => n + " B",
}
vm.createContext(ctx)
vm.runInContext(extract("oldExportsWorthOffering") + extract("oldExportsListHtml"), ctx)

const file = (kind, filename, keptFilename) => ({
  kind,
  filename,
  keptFilename,
  modifiedMs: Date.UTC(2026, 2, 20),
  bytes: 12,
})

// Nothing older, nothing to ask.
assert.strictEqual(ctx.oldExportsWorthOffering([], new Set()), false)

// "Not now" holds for those files, so Refresh does not ask again…
const a = file("Articles", "a-old.json", "a-new.json")
assert.strictEqual(ctx.oldExportsWorthOffering([a], new Set()), true)
assert.strictEqual(ctx.oldExportsWorthOffering([a], new Set(["a-old.json"])), false)

// …but a file that has only now become old is a new question.
const b = file("Articles", "a-newer-old.json", "a-new.json")
assert.strictEqual(ctx.oldExportsWorthOffering([a, b], new Set(["a-old.json"])), true)

// Each kind says once what stays, and every file is on its own row.
const html = ctx.oldExportsListHtml([
  a,
  b,
  file("Dialogs", "d-old.json", "d-new.json"),
])
assert.strictEqual(html.match(/old-exports-kind/g).length, 2)
assert.strictEqual(html.match(/old-exports-row/g).length, 3)
assert.ok(html.includes("keeping a-new.json"))
assert.ok(html.includes("keeping d-new.json"))
assert.ok(html.includes("2026-03-20 · 12 B"))

// File names are data.
const nasty = ctx.oldExportsListHtml([file("Articles", "<img src=x>.json", "<b>.json")])
assert.ok(!nasty.includes("<img"))
assert.ok(!nasty.includes("<b>"))

// A file whose time could not be read still gets a row.
const undated = ctx.oldExportsListHtml([{ ...a, modifiedMs: null }])
assert.ok(undated.includes(">12 B<"))

console.log("old-exports: ok")

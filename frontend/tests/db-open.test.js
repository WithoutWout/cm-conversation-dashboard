// Opening the conversations database: one open at a time, and every read
// waits for it. See docs/loading-states.md → "The database is still opening".
//
// Before this, the launch opened the saved database in the background and
// nothing waited for it: a read in the meantime failed with "No database
// open.", and entering Conversations started a second open of the same file.
// The real functions run here against a scripted `set_db_path`.
const assert = require("assert")
const vm = require("vm")
const { extract } = require("./extract")

const ctx = vm.createContext({ Promise })
vm.runInContext(
  `
  let _convDbOpening = null
  let convDbOpenedPath = ""
  let convDbPhaseLabel = ""
  const log = []
  const pending = []
  let changes = 0
  // Each invoke is held until the test lets it go.
  const _rawSetDbPath = (path) =>
    new Promise((resolve, reject) => {
      log.push("open " + path)
      pending.push({ path, resolve, reject })
    })
  function convDbOpeningChanged() { changes++ }
  ${["convDbIsOpening", "convDbOpen", "convDbWhenReady", "convDbWaitText"].map(extract).join("\n")}
  globalThis.api = {
    convDbIsOpening, convDbOpen, convDbWhenReady, convDbWaitText,
    log, pending,
    opened: () => convDbOpenedPath,
    changes: () => changes,
  }
  `,
  ctx,
)
const api = ctx.api
const tick = () => new Promise((r) => setTimeout(r, 0))

let failed = 0
const tests = []
const test = (name, fn) => tests.push([name, fn])

test("a second open of the same file joins the first", async () => {
  const a = api.convDbOpen("/db/a")
  const b = api.convDbOpen("/db/a")
  assert.strictEqual(a, b)
  await tick()
  assert.deepStrictEqual([...api.log], ["open /db/a"])
  assert.ok(api.convDbIsOpening())
  api.pending.shift().resolve()
  await a
  assert.ok(!api.convDbIsOpening())
  assert.strictEqual(api.opened(), "/db/a")
})

test("a read waits for the open in flight, then goes", async () => {
  api.convDbOpen("/db/b")
  let read = false
  const r = api.convDbWhenReady().then(() => (read = true))
  await tick()
  assert.strictEqual(read, false, "the read went out before the database was open")
  api.pending.shift().resolve()
  await r
  assert.strictEqual(read, true)
})

test("a different file waits for the current open to settle", async () => {
  api.log.length = 0
  const first = api.convDbOpen("/db/c")
  const second = api.convDbOpen("/db/d")
  await tick()
  assert.deepStrictEqual([...api.log], ["open /db/c"], "two opens ran at once")
  api.pending.shift().resolve()
  await first
  await tick()
  assert.deepStrictEqual([...api.log], ["open /db/c", "open /db/d"])
  assert.ok(api.convDbIsOpening(), "the second open is still running")
  api.pending.shift().resolve()
  await second
  assert.strictEqual(api.opened(), "/db/d")
})

test("a failed open rejects its callers, releases its readers, and is not 'open'", async () => {
  const p = api.convDbOpen("/db/gone")
  const ready = api.convDbWhenReady()
  await tick()
  api.pending.shift().reject(new Error("no such file"))
  await assert.rejects(p, /no such file/)
  await ready // resolves: the read then fails on its own, with the backend's words
  assert.ok(!api.convDbIsOpening())
  assert.strictEqual(api.opened(), "/db/d", "still the last database that did open")
})

test("nothing opening: a read goes at once, and the pane keeps its own words", async () => {
  await api.convDbWhenReady()
  const plain = api.convDbWaitText("Reading…", "", false, "")
  assert.strictEqual(plain.label, "Reading…")
  const waiting = api.convDbWaitText("Reading…", "", true, "")
  assert.strictEqual(waiting.label, "Opening the database…")
  assert.ok(waiting.note)
  const migrating = api.convDbWaitText("Reading…", "", true, "Recounting interactions per conversation…")
  assert.strictEqual(migrating.note, "Recounting interactions per conversation…")
})

;(async () => {
  for (const [name, fn] of tests) {
    try {
      await fn()
      console.log("  ok   " + name)
    } catch (e) {
      failed++
      console.log("  FAIL " + name + "\n       " + e.message)
    }
  }
  if (failed) {
    console.log("\n" + failed + " database-open test(s) failed")
    process.exit(1)
  }
  console.log("\nDatabase open: all checks passed")
})()

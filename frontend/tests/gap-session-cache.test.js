// Analysis keeps the conversations it has opened, so stepping back to a row
// does not read it again. Three things that cache must never do: remember a
// failed read (which showed that conversation as empty for the rest of the
// session), answer from a database other than the one open, or grow without
// bound. `gapAiSelect` is run for real against a stubbed backend; the Recognition
// and Feedback panes share its caching lines.
const assert = require("assert")
const fs = require("fs")
const path = require("path")
const vm = require("vm")
const { extract } = require("./extract")

const src = fs.readFileSync(path.join(__dirname, "..", "index.html"), "utf8")
const constLine = (name) => {
  const m = new RegExp("const " + name + " = [^\\n]+").exec(src)
  if (!m) throw new Error("not found: " + name)
  return m[0]
}

const reads = []
let failNext = false
const shown = []
const ctx = vm.createContext({
  window: {
    backend: {
      getSessionInteractions: async (uuid) => {
        reads.push(uuid)
        if (failNext) {
          failNext = false
          throw new Error("interrupted")
        }
        return [{ sessionUuid: uuid }]
      },
    },
  },
  document: { getElementById: () => ({ querySelector: () => null }) },
  requestAnimationFrame: () => {},
  setPaneLoading: () => {},
  clearPaneLoading: () => {},
  gapAiPaint: () => {},
  gapAiRenderSide: () => {},
  gapShowThread: (thread, rows) => shown.push(rows),
})
vm.runInContext(
  [
    "var gapSessionCache = new Map()",
    "var gapSessionCacheDb = null",
    "var convDbPath = 'a.db'",
    "var gapAiActive = null",
    "var gapAiRows = []",
    constLine("GAP_SESSION_CACHE_MAX"),
    extract("gapCachedSession"),
    extract("gapCacheSession"),
    // `extract` starts at the `function` keyword, so the `async` is restored here.
    "async " + extract("gapAiSelect"),
    "globalThis.MAX = GAP_SESSION_CACHE_MAX",
  ].join("\n"),
  ctx,
)
const run = (code) => vm.runInContext(code, ctx)
const open = async (uuid, logId) => {
  run(`gapAiRows = [{ logId: ${logId}, sessionUuid: ${JSON.stringify(uuid)} }]`)
  await run(`gapAiSelect(${logId})`)
}

let failed = 0
async function check(name, fn) {
  try {
    await fn()
    console.log("  ok   " + name)
  } catch (e) {
    failed++
    console.log("  FAIL " + name + "\n       " + e.message)
  }
}

;(async () => {
  await check("a conversation read once is answered from the cache", async () => {
    reads.length = 0
    await open("s1", 1)
    await open("s1", 2)
    assert.deepStrictEqual(reads, ["s1"])
  })

  await check("a failed read is not cached, so the next visit reads again", async () => {
    reads.length = 0
    shown.length = 0
    failNext = true
    await open("s2", 3)
    // As JSON: the empty array is made inside the VM, whose Array is not ours.
    assert.strictEqual(JSON.stringify(shown[0]), "[]", "the failed read still shows an empty thread")
    await open("s2", 4)
    assert.deepStrictEqual(reads, ["s2", "s2"])
    assert.strictEqual(JSON.stringify(shown[1]), '[{"sessionUuid":"s2"}]')
  })

  await check("another database never answers from this one's cache", async () => {
    reads.length = 0
    run("convDbPath = 'b.db'")
    await open("s1", 5)
    assert.deepStrictEqual(reads, ["s1"])
    run("convDbPath = 'a.db'")
    await open("s1", 6)
    assert.deepStrictEqual(reads, ["s1", "s1"], "switching back must not resurrect b.db's rows")
  })

  await check("the cache is bounded, dropping the oldest read first", async () => {
    const max = run("MAX")
    for (let i = 0; i < max + 5; i++) await open("bulk" + i, 1000 + i)
    assert.strictEqual(run("gapSessionCache.size"), max)
    reads.length = 0
    await open("bulk0", 9000)
    await open("bulk" + (max + 4), 9001)
    assert.deepStrictEqual(reads, ["bulk0"], "the oldest was evicted, the newest kept")
  })

  if (failed) {
    console.log(`GAP session cache: ${failed} check(s) failed`)
    process.exit(1)
  }
  console.log("GAP session cache: all checks passed")
})()

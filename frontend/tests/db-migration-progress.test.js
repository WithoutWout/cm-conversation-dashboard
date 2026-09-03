// The overlay during a first open after an upgrade.
//
// `openDbAtPath` normally finishes in well under a second and says "Opening
// database…" the whole time, which is fine. A database that has not been
// migrated yet is a different operation: interning the stored contexts rewrites
// every row and then compacts the file — 19-64 s on an 824 MB database — and a
// spinner that spends all of it on the same sentence reads as a hang.
//
// What can actually go wrong here is not visible by opening a database once:
// subscribing after the invoke (so the first phase is missed), leaving the
// listener registered (so a later open is relabelled by a migration that is no
// longer running), or a phase name with no wording (so the label goes blank
// mid-migration). Each is a one-line slip and none of them show up unless a
// migration is in flight, which on a given machine happens exactly once. So the
// real function runs here against a scripted event stream.
const { extract } = require("./extract")
const fs = require("fs")
const path = require("path")
const vm = require("vm")

// The wording table is a top-level `const`, which `extract` (a matcher for
// `function` declarations) cannot see. Sliced out of the real source rather
// than restated here — a copy of the labels in the test would keep passing
// after someone reworded the ones on screen.
const src = fs.readFileSync(path.join(__dirname, "..", "index.html"), "utf8")
const LABELS_DECL = (() => {
  const at = src.indexOf("      const DB_MIGRATION_LABELS = {")
  if (at < 0) throw new Error("could not find DB_MIGRATION_LABELS in index.html")
  const end = src.indexOf("\n      }\n", at)
  if (end < 0) throw new Error("could not find the end of DB_MIGRATION_LABELS")
  return src.slice(at, end + "\n      }".length)
})()

const out = []
let failed = 0
const ok = (label, cond) => {
  out.push(`  ${cond ? "ok  " : "FAIL"} ${label}`)
  if (!cond) failed++
}

// ── The harness ────────────────────────────────────────────────────────────
//
// Everything `openDbAtPath` touches that is not the subject of this test is a
// recording stub. `labels` is what the test is actually about: every string the
// overlay was ever set to, in order.
function harness({ emitDuring, listenThrows = false, failOpen = false }) {
  const labels = []
  let handler = null
  let unlistenCalls = 0
  let listenerLive = false

  const ctx = vm.createContext({
    console,
    Promise,
    Number,
    setTimeout,
    localStorage: { setItem() {}, getItem: () => null },
    CONV_DB_STORAGE_KEY: "conv-db-path",
    // The subject.
    setConvOverlay: (visible, label) => {
      if (visible && label) labels.push(label)
    },
    setConvDbBusy() {},
    yieldToPaint: () => Promise.resolve(),
    updateConvDbStatus() {},
    initDatePicker() {},
    showImportToast() {},
    loadSessions: () => Promise.resolve(),
    convDbPath: null,
    contextOptionsLoaded: true,
    metadataOptionsLoaded: true,
    convDatePickerInited: true,
    window: {
      backend: {
        onDbMigrating: (h) => {
          if (listenThrows) return Promise.reject(new Error("no listener"))
          handler = h
          listenerLive = true
          return Promise.resolve(() => {
            unlistenCalls++
            listenerLive = false
          })
        },
        // The migration reports while the invoke is in flight, which is the
        // whole shape of the thing: one long call that talks back.
        setDbPath: () =>
          new Promise((resolve, reject) => {
            for (const ev of emitDuring || []) {
              if (handler) handler({ payload: ev })
            }
            failOpen ? reject(new Error("boom")) : resolve()
          }),
        getDateRange: () => Promise.resolve({ min: "", max: "" }),
      },
    },
  })

  // `extract` matches on `function <name>(`, so it slices from inside
  // `async function openDbAtPath(` and drops the keyword. Put it back rather
  // than teach the extractor about modifiers — if the function ever stops
  // being async this throws a syntax error on the next run, which is a loud
  // failure rather than a quiet one.
  vm.runInContext(LABELS_DECL + "\nasync " + extract("openDbAtPath"), ctx)
  return ctx
    .openDbAtPath("/tmp/x.db")
    .then((okResult) => ({
      labels,
      unlistenCalls,
      listenerLive: () => listenerLive,
      okResult,
    }))
}

// ── A migration reports; the overlay follows it ────────────────────────────
harness({
  emitDuring: [
    { phase: "answerIndex" },
    { phase: "contexts", done: 0 },
    { phase: "contexts", done: 20000 },
    { phase: "contexts", done: 147963 },
    { phase: "compacting" },
  ],
})
  .then((r) => {
    ok(
      "the overlay starts on the ordinary open label",
      r.labels[0] === "Opening database…",
    )
    ok(
      "each phase relabels it",
      r.labels.some((l) => l.startsWith("Indexing what answered")) &&
        r.labels.some((l) => l.startsWith("Reorganising stored")) &&
        r.labels.some((l) => l.startsWith("Reclaiming disk space")),
    )
    // The count is the only thing on screen that visibly moves, so it has to
    // reach the label — and it has to be readable, not a raw 147963.
    ok(
      "the running count reaches the label, grouped",
      r.labels.some((l) => l.includes("147,963 conversations moved")),
    )
    // A zero count would read as "nothing is happening", which is the opposite
    // of what the first event means.
    ok(
      "the opening report shows no count rather than zero",
      r.labels.some(
        (l) => l.startsWith("Reorganising stored") && !l.includes("0 conv"),
      ),
    )
    ok(
      "every phase says it runs only once",
      r.labels
        .slice(1)
        .every((l) => l.endsWith("This runs once.")),
    )
    ok("the listener is torn down", r.unlistenCalls === 1 && !r.listenerLive())

    // ── An open with nothing to migrate says nothing extra ────────────────
    return harness({ emitDuring: [] })
  })
  .then((r) => {
    ok(
      "an already-migrated database keeps the ordinary label",
      r.labels.length === 1 && r.labels[0] === "Opening database…",
    )
    ok("and still tears the listener down", r.unlistenCalls === 1)

    // ── An unknown phase must not blank the label ─────────────────────────
    return harness({ emitDuring: [{ phase: "somethingNewer" }, {}] })
  })
  .then((r) => {
    ok(
      "a phase this build has no wording for is ignored, not shown blank",
      r.labels.length === 1 && r.labels[0] === "Opening database…",
    )

    // ── A failed open must not leak the listener ──────────────────────────
    return harness({ emitDuring: [{ phase: "contexts", done: 5 }], failOpen: true })
  })
  .then((r) => {
    ok(
      "a failed open still tears the listener down",
      r.unlistenCalls === 1 && !r.listenerLive(),
    )
    ok("and reports failure to the caller", r.okResult === false)

    // ── No event bridge at all (older shell) must not break opening ───────
    return harness({ emitDuring: [], listenThrows: true })
  })
  .then((r) => {
    ok(
      "an unavailable event bridge does not stop the database opening",
      r.okResult === true && r.labels[0] === "Opening database…",
    )
    ok("and there is nothing to tear down", r.unlistenCalls === 0)
  })
  .then(() => {
    console.log(out.join("\n"))
    console.log(
      failed
        ? `\nDB migration progress: ${failed} FAILED`
        : "\nDB migration progress: all checks passed",
    )
    process.exit(failed ? 1 : 0)
  })
  .catch((e) => {
    console.log(out.join("\n"))
    console.error("\nDB migration progress: threw —", e)
    process.exit(1)
  })

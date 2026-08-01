// The settings backup deliberately carries live credentials, which makes two
// things load-bearing: that the export list says so on purpose rather than by
// accident, and that importing a file can only ever write keys this build
// knows about. A crafted backup must not be able to set arbitrary
// localStorage entries.
const { extract } = require("./extract")
const vm = require("vm")
const fs = require("fs")
const path = require("path")

const src = fs.readFileSync(path.join(__dirname, "..", "index.html"), "utf8")

// The two lists are plain array literals; slice them out and evaluate them so
// the test reads the real declarations rather than a copy that can drift.
function arrayLiteral(name) {
  const at = src.indexOf(`const ${name} = [`)
  if (at === -1) throw new Error("not found: " + name)
  const start = src.indexOf("[", at)
  let depth = 0
  for (let i = start; i < src.length; i++) {
    if (src[i] === "[") depth++
    else if (src[i] === "]" && --depth === 0) {
      // Strip line comments: the entries carry trailing `// why` notes.
      return eval(src.slice(start, i + 1).replace(/\/\/[^\n]*/g, ""))
    }
  }
  throw new Error("unbalanced: " + name)
}

const KEYS = arrayLiteral("SETTINGS_EXPORT_KEYS").map((k) => k.key)
const EXCLUDED = arrayLiteral("SETTINGS_EXPORT_EXCLUDED")

const ctx = vm.createContext({ console })
vm.runInContext(
  `
  const SETTINGS_EXPORT_KEYS = ${JSON.stringify(arrayLiteral("SETTINGS_EXPORT_KEYS"))}
  let _status = null
  const _store = new Map()
  const localStorage = { setItem: (k, v) => _store.set(k, v) }
  const location = { reload() { _reloaded = true } }
  let _reloaded = false
  function setTimeout() {}          // the reload is not what is under test
  function _settingsIoStatus(msg, tone) { _status = { msg, tone } }
  ${extract("_applyImportedSettings")}
  function reset() { _store.clear(); _status = null; _reloaded = false }
  function stored() { return Object.fromEntries(_store) }
  function status() { return _status }
`,
  ctx,
)

const out = []
let failed = 0
const ok = (n, c) => {
  if (!c) failed++
  out.push((c ? "  PASS  " : "  FAIL  ") + n)
}

// ── What the file is allowed to contain ─────────────────────────────────────
ok("the bearer token is exported", KEYS.includes("cm-analytics-token"))
ok("the bearer token is not also excluded", !EXCLUDED.includes("cm-analytics-token"))
ok("the analytics config is exported", KEYS.includes("cm-analytics-config"))
ok("context URLs are exported", KEYS.includes("cm-base-url") && KEYS.includes("halo-base-url"))

// Paths stay out for a different reason than credentials did: restoring one on
// another machine points the app at something that isn't there.
ok("machine-specific paths stay excluded", EXCLUDED.includes("cm-conv-db-path") && EXCLUDED.includes("cm-data-folder"))
ok("the dev toggle stays excluded", EXCLUDED.includes("cm-perf-debug"))
ok("no key is both exported and excluded", !KEYS.some((k) => EXCLUDED.includes(k)))

// The list is an allowlist; a duplicate would export a key twice and hint the
// list is being appended to without being read.
ok("the export list has no duplicates", new Set(KEYS).size === KEYS.length)

// ── Importing can only write keys this build knows ──────────────────────────
ctx.reset()
ctx._applyImportedSettings(
  { "cm-base-url": "https://restored.test/", "totally-made-up": "x", __proto__: "y" },
  "0.12.0",
  false,
)
const stored = ctx.stored()
ok("a known key is restored", stored["cm-base-url"] === "https://restored.test/")
ok("an unknown key is refused", !("totally-made-up" in stored))
ok("only the known key was written", Object.keys(stored).length === 1)

// Every managed key is stored as a string; writing an object would make the
// reader that parses it throw at startup, which reads as the app breaking.
ctx.reset()
ctx._applyImportedSettings({ "cm-collections": [{ id: 1 }] }, "0.12.0", false)
ok(
  "a non-string value is serialised, not stored raw",
  ctx.stored()["cm-collections"] === '[{"id":1}]',
)

// ── Credentials-only restores ───────────────────────────────────────────────
// A backup whose localStorage half is empty but which carried credentials is a
// real restore, not "that file contained no settings this app uses".
ctx.reset()
ctx._applyImportedSettings({}, "0.12.0", true)
ok("a credentials-only restore is not reported as empty", ctx.status().tone === "ok")
ok("the restore says credentials came back", /Analytics API credentials/.test(ctx.status().msg))

ctx.reset()
ctx._applyImportedSettings({}, "0.12.0", false)
ok("a genuinely empty file is still an error", ctx.status().tone === "error")

ctx.reset()
ctx._applyImportedSettings({ "cm-base-url": "x", nope: 1 }, "0.12.0", false)
ok("unknown keys are counted in the report", /ignored 1 unknown/.test(ctx.status().msg))

console.log(out.join("\n"))
console.log(
  failed ? `\nSettings backup: ${failed} check(s) failed` : "\nSettings backup: all checks passed",
)
process.exit(failed ? 1 : 0)

// The update modal is the only place in the app whose primary button can be
// wrong in a way the user cannot recover from: offering "Install and restart"
// on a copy that cannot replace itself leaves them clicking a button that
// always fails, with the working manual download hidden behind it. These tests
// pin that the two states are driven entirely by `canSelfUpdate`.
const { extract } = require("./extract")
const vm = require("vm")

const NAMES = ["openUpdateModal", "closeUpdateModal"]

// Minimal stand-in for the elements openUpdateModal touches. Only the
// properties the function actually sets — anything else it reaches for should
// fail the test loudly rather than be silently absorbed by a Proxy.
function makeEl(id) {
  return {
    id,
    textContent: "",
    innerHTML: "",
    hidden: false,
    disabled: false,
    className: "",
    style: {},
    classList: {
      _set: new Set(),
      add(c) { this._set.add(c) },
      remove(c) { this._set.delete(c) },
      contains(c) { return this._set.has(c) },
    },
  }
}

const IDS = [
  "updateModal", "updateModalVersion", "updateModalVersionFrom", "updateModalNotes",
  "updateModalInstallBtn", "updateModalDownloadBtn", "updateModalHint",
  "updateModalProgress", "updateModalProgressFill", "updateModalProgressText",
  "updateModalError",
]

const ctx = vm.createContext({ console })
vm.runInContext(`
  const UPDATE_RELEASES_URL = "https://example.invalid/releases/latest"
  let lastUpdateCheck = null
  let updateInstalling = false
  const _els = new Map()
  const document = {
    getElementById(id) {
      if (!_els.has(id)) throw new Error("unexpected element id: " + id)
      return _els.get(id)
    },
  }
  ${NAMES.map(extract).join("\n")}
  function reset(makeEl, ids) { _els.clear(); for (const id of ids) _els.set(id, makeEl(id)) }
  function el(id) { return _els.get(id) }
  function setInstalling(v) { updateInstalling = v }
  function getLastCheck() { return lastUpdateCheck }
`, ctx)

const out = []
let failed = 0
const ok = (n, c) => { if (!c) failed++; out.push((c ? "  PASS  " : "  FAIL  ") + n) }

const open = (res, current) => {
  ctx.reset(makeEl, IDS)
  ctx.setInstalling(false)
  ctx.openUpdateModal(res, current)
}
const el = (id) => ctx.el(id)

// ── A portable copy that can replace itself ─────────────────────────────────
open(
  { status: "available", version: "0.12.0", canSelfUpdate: true, mode: "portable", notes: "Fixed a thing" },
  "0.11.1",
)
ok("the modal opens", el("updateModal").classList.contains("open"))
ok("both versions are shown", el("updateModalVersionFrom").textContent === "v0.11.1" && el("updateModalVersion").textContent === "v0.12.0")
ok("Install is the offered action", el("updateModalInstallBtn").hidden === false)
ok("the manual download stays out of the way", el("updateModalDownloadBtn").hidden === true)
ok("the hint says no admin rights are needed", /administrator rights/i.test(el("updateModalHint").textContent))
ok("progress starts hidden and empty", el("updateModalProgress").hidden === true && el("updateModalProgressFill").style.width === "0%")
ok("no error is shown on open", el("updateModalError").hidden === true)
ok("the check is remembered", ctx.getLastCheck().version === "0.12.0")

// ── A copy that cannot: read-only folder, or a dev build ────────────────────
open(
  {
    status: "available", version: "0.12.0", canSelfUpdate: false, mode: "portable",
    blockedReason: "This copy is in a folder it cannot write to (D:\\readonly).",
  },
  "0.11.1",
)
ok("Install is not offered", el("updateModalInstallBtn").hidden === true)
ok("the manual download takes over", el("updateModalDownloadBtn").hidden === false)
ok("the hint is the reason from the backend", el("updateModalHint").textContent.includes("cannot write to"))

// A blocked state with no reason must still say something actionable — an
// empty hint next to a lone "Download manually" button explains nothing.
open({ status: "available", version: "0.12.0", canSelfUpdate: false }, "0.11.1")
ok("a missing reason falls back to an explanation", el("updateModalHint").textContent.length > 20)

// ── Release notes are release-author text, not markup ───────────────────────
open(
  { status: "available", version: "0.12.0", canSelfUpdate: true, notes: "<img src=x onerror=alert(1)>" },
  "0.11.1",
)
ok("notes go in as text, never as HTML", el("updateModalNotes").textContent === "<img src=x onerror=alert(1)>" && el("updateModalNotes").innerHTML === "")
ok("notes are shown when present", el("updateModalNotes").hidden === false)

open({ status: "available", version: "0.12.0", canSelfUpdate: true }, "0.11.1")
ok("the notes box is hidden when the release has none", el("updateModalNotes").hidden === true)
open({ status: "available", version: "0.12.0", canSelfUpdate: true, notes: "   \n  " }, "0.11.1")
ok("whitespace-only notes count as none", el("updateModalNotes").hidden === true)

// ── Reopening must not carry the previous attempt's failure over ────────────
open({ status: "available", version: "0.12.0", canSelfUpdate: true }, "0.11.1")
el("updateModalError").hidden = false
el("updateModalProgress").hidden = false
el("updateModalInstallBtn").disabled = true
ctx.openUpdateModal({ status: "available", version: "0.12.0", canSelfUpdate: true }, "0.11.1")
ok("a reopen clears a previous error", el("updateModalError").hidden === true)
ok("a reopen clears previous progress", el("updateModalProgress").hidden === true)
ok("a reopen re-enables Install", el("updateModalInstallBtn").disabled === false)

// ── The window cannot be dismissed mid-install ──────────────────────────────
// The swap has already renamed the running executable by then; closing the
// modal would suggest it had been called off.
open({ status: "available", version: "0.12.0", canSelfUpdate: true }, "0.11.1")
ctx.setInstalling(true)
ctx.closeUpdateModal()
ok("closing is refused while installing", el("updateModal").classList.contains("open"))
ctx.setInstalling(false)
ctx.closeUpdateModal()
ok("closing works once the install is not running", !el("updateModal").classList.contains("open"))

console.log(out.join("\n"))
console.log(failed ? `\nUpdate modal: ${failed} check(s) failed` : "\nUpdate modal: all checks passed")
process.exit(failed ? 1 : 0)

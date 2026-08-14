// The message-metadata bubble is anchored to the tag button that opened it, and
// the thing that can go wrong is not "it fails to open" — it is that the bubble
// ends up somewhere plausible while its tail points at nothing. That happens
// only in the cases nobody reaches by hand: a message near the bottom of the
// thread, a message at the very edge of a narrow window, a bubble too tall for
// either side of its anchor.
//
// `msgMetaPlacement` is pure arithmetic over three rects precisely so those
// cases can be asserted here instead of hunted for on screen.
const { extract } = require("./extract")
const vm = require("vm")

const NAMES = ["msgMetaPlacement"]

const ctx = vm.createContext({ console, Math })
vm.runInContext(
  `
  const MSG_META_GAP = 9
  const MSG_META_EDGE = 10
  const MSG_META_TAIL_INSET = 16
  ${NAMES.map(extract).join("\n")}
`,
  ctx,
)
const place = ctx.msgMetaPlacement

const GAP = 9
const EDGE = 10
const INSET = 16

const WIN = { width: 1280, height: 800 }
const BOX = { width: 420, height: 300 }

/// A tag button, 18×18, at a given top-left.
const btn = (left, top) => ({
  left,
  top,
  width: 18,
  height: 18,
  bottom: top + 18,
  right: left + 18,
})

const out = []
let failed = 0
const ok = (n, c) => {
  if (!c) failed++
  out.push((c ? "  PASS  " : "  FAIL  ") + n)
}
/// The tail's absolute position — where it actually points on screen.
const tailAt = (p) => p.left + p.tailX

// ── The ordinary case ───────────────────────────────────────────────
{
  const a = btn(600, 300)
  const p = place(a, BOX, WIN)
  ok("opens below when there is room", !p.above)
  ok("sits a gap under the button", p.top === a.bottom + GAP)
  ok("is centred on the button", p.left === a.left + a.width / 2 - BOX.width / 2)
  ok("tail points at the button's centre", tailAt(p) === a.left + a.width / 2)
}

// ── Flipping above ──────────────────────────────────────────────────
{
  // A message near the bottom of the thread: 40px of room below, plenty above.
  const a = btn(600, WIN.height - 58)
  const p = place(a, BOX, WIN)
  ok("flips above with no room below", p.above)
  ok("sits a gap over the button", p.top === a.top - GAP - BOX.height)
  ok("still points at the button's centre", tailAt(p) === a.left + a.width / 2)
}
{
  // Room below is tight but there is even less above — stay below rather than
  // flip into a worse spot.
  const a = btn(600, 20)
  const p = place(a, { width: 420, height: 700 }, WIN)
  ok("does not flip when above is worse than below", !p.above)
}
{
  // Exactly enough room below, to the pixel. This is the boundary the
  // comparison is written around, so it is worth pinning.
  const h = 300
  const a = btn(600, WIN.height - h - GAP - EDGE - 18)
  ok("stays below when it fits by exactly one pixel", !place(a, { width: 420, height: h }, WIN).above)
  ok(
    "flips when it is one pixel short",
    place(btn(600, a.top + 1), { width: 420, height: h }, WIN).above,
  )
}

// ── Clamped to the window, tail still on target ─────────────────────
{
  // A button hard against the left edge: the box cannot be centred on it.
  const a = btn(4, 300)
  const p = place(a, BOX, WIN)
  ok("never crosses the left edge", p.left === EDGE)
  ok("tail follows the button when clamped left", tailAt(p) === INSET + EDGE)
  ok("tail stays clear of the corner", p.tailX >= INSET)
}
{
  const a = btn(WIN.width - 22, 300)
  const p = place(a, BOX, WIN)
  ok("never crosses the right edge", p.left + BOX.width === WIN.width - EDGE)
  ok("tail stays clear of the far corner", p.tailX <= BOX.width - INSET)
  ok(
    "tail is as close to the button as the corner allows",
    tailAt(p) === p.left + BOX.width - INSET,
  )
}
{
  // Anchored mid-window but close enough to the edge that only the tail moves.
  const a = btn(WIN.width - 240, 300)
  const p = place(a, BOX, WIN)
  ok(
    "an anchor inside the clamp still gets an exact tail",
    tailAt(p) === a.left + a.width / 2,
  )
}

// ── Degenerate windows ──────────────────────────────────────────────
{
  // A window narrower than the bubble. The left clamp must win over the right
  // one, or `left` goes negative and the bubble hangs off the screen.
  const tiny = { width: 300, height: 800 }
  const p = place(btn(150, 300), BOX, tiny)
  ok("a box wider than the window still starts on screen", p.left === EDGE)
}
{
  // No room on either side of the anchor. It has to land somewhere; what it
  // must not do is go off the top.
  const short = { width: 1280, height: 200 }
  const p = place(btn(600, 150), { width: 420, height: 400 }, short)
  ok("never goes off the top of the window", p.top >= EDGE)
}
{
  // A bubble narrower than two tail insets — the two clamps would cross.
  const p = place(btn(600, 300), { width: 20, height: 100 }, WIN)
  ok("a very narrow bubble still yields a tail on the box", p.tailX >= 0 && p.tailX <= 20)
}

console.log(out.join("\n"))
console.log(
  failed
    ? `\nMessage metadata placement: ${failed} FAILED`
    : "\nMessage metadata placement: all checks passed",
)
process.exit(failed ? 1 : 0)

/**
 * What colour a provider's mark is drawn in.
 *
 * Port of the original's `BotMarkTint` — same table, same wheel, same
 * arithmetic. The mark stands where the provider's logo stood, so its colour is
 * **identity**, not status: that is what keeps it clear of `UsageTint`, where
 * colour means one thing only, how close a limit is. The ring around the mark
 * still carries that, unchanged.
 *
 * **It is still a risk worth naming.** Rings were drawn in brand colours once
 * and it went badly, because Claude Code's orange-red at 3% used looked like a
 * warning. A brand colour on the disc is a weaker version of the same hazard:
 * the ring is the thing that turns red, but a red-ish body inside a green ring
 * is two colours disagreeing. If that reads badly on a real rail, the fix is
 * this table, not the ring.
 */

interface Rgb {
  r: number
  g: number
  b: number
}

/**
 * Brand colours, for the providers that have one.
 *
 * `null` is not "unknown", it is **monochrome by design** — OpenAI, Cursor,
 * GitHub, xAI, Ollama, OpenCode and the rest draw a black-or-white glyph and
 * nothing else. Those are dealt a colour of Pulse's own; see `deal`.
 */
const BRAND: Record<string, number> = {
  'claude-code': 0xd97757,
  deepseek: 0x4d6bfe,
  // Best-guess brand colours: right family, exact value unconfirmed.
  minimax: 0xe8483f,
  'minimax-cn': 0xe8483f,
  'glm-coding': 0x3a7bf7,
  'kimi-code': 0x7aa5ff,
  'hugging-face': 0xffd21e,
  volcengine: 0x1664ff,
}

/**
 * Monochrome by design: no brand colour, so the wheel deals one.
 *
 * The original's `brand(for:)` names these explicitly rather than letting them
 * fall through, because "has no colour" and "has a colour we have not written
 * down yet" are different claims and only the first one is safe to deal from.
 */
export const MONOCHROME: ReadonlySet<string> = new Set([
  'codex',
  'cursor',
  'opencode-go',
  'zai',
  'copilot',
  'grok',
  'v2ex',
  'kiro',
  'command-code',
  'new-api',
])


/** The brand colour for a provider, or `null` when it has none. */
export function brandColour(providerId: string): string | null {
  const hex = BRAND[providerId.split('--account-')[0]]
  return hex === undefined ? null : toCss(fromHex(hex))
}

/**
 * The wheel's size. **Prime on purpose** — `deal` walks the wheel with a stride
 * coprime to its size so every colour is visited once, and a prime size means
 * every stride qualifies rather than a handful. It is deliberately ahead of the
 * count of colourless providers, because being *exactly* that count is how it
 * overflowed once already.
 */
export const PALETTE_SIZE = 17

/** The offset that keeps the wheel as a whole furthest from the brand hues. */
const WHEEL_HUE_OFFSET = 27
const WHEEL_SATURATION = 0.68
const WHEEL_TARGET_LUMINANCE = 0.62

/** Where a body colour stops reading against the disc behind it. */
const LUMINANCE_FLOOR = 0.42

/** `--fg` upstream, and the eyes on a lifted body. */
export const INK = '#0f0f0f'
/** `--bg` upstream, and the eyes on a colour dark enough to need them. */
export const PAPER = '#f7f7f7'

function fromHex(hex: number): Rgb {
  return {
    r: ((hex >> 16) & 0xff) / 255,
    g: ((hex >> 8) & 0xff) / 255,
    b: (hex & 0xff) / 255,
  }
}

function toCss({ r, g, b }: Rgb): string {
  const channel = (value: number) => Math.round(Math.max(0, Math.min(1, value)) * 255)
  return `rgb(${channel(r)} ${channel(g)} ${channel(b)})`
}

/** `BotMarkPalette.hsl`: standard HSL, one hue, one saturation, one lightness. */
export function hsl(degrees: number, saturation: number, lightness: number): Rgb {
  const h = (((degrees % 360) + 360) % 360) / 360
  const s = saturation
  const l = lightness

  if (s === 0) return { r: l, g: l, b: l }

  const q = l < 0.5 ? l * (1 + s) : l + s - l * s
  const p = 2 * l - q
  const channel = (t: number) => {
    let value = t
    if (value < 0) value += 1
    if (value > 1) value -= 1
    if (value < 1 / 6) return p + (q - p) * 6 * value
    if (value < 1 / 2) return q
    if (value < 2 / 3) return p + (q - p) * (2 / 3 - value) * 6
    return p
  }
  return { r: channel(h + 1 / 3), g: channel(h), b: channel(h - 1 / 3) }
}

/** Relative luminance, the same weights the original uses. */
export function luminance({ r, g, b }: Rgb): number {
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

/** The hue of a colour in degrees, for the wheel's spacing arithmetic. */
export function hueOf({ r, g, b }: Rgb): number {
  const max = Math.max(r, g, b)
  const min = Math.min(r, g, b)
  const delta = max - min
  if (delta === 0) return 0
  let hue: number
  if (max === r) hue = ((g - b) / delta) % 6
  else if (max === g) hue = (b - r) / delta + 2
  else hue = (r - g) / delta + 4
  hue *= 60
  return hue < 0 ? hue + 360 : hue
}

/**
 * The lightness that puts this hue at `target` luminance, by bisection.
 *
 * **Levelling matters as much as spacing.** At one fixed lightness a yellow
 * comes out glaring and a blue nearly black, so each hue's lightness is solved
 * for the same perceived luminance instead. Twenty steps is far more than the
 * eye needs and costs nothing — the wheel is built once.
 */
export function levelled(hue: number, saturation: number, target: number): Rgb {
  let low = 0
  let high = 1
  let colour = hsl(hue, saturation, 0.5)
  for (let step = 0; step < 20; step++) {
    const middle = (low + high) / 2
    colour = hsl(hue, saturation, middle)
    if (luminance(colour) < target) low = middle
    else high = middle
  }
  return colour
}

/**
 * Where a body colour stops reading against the disc behind it: enough of the
 * way to white to clear the floor, and no further.
 */
export function lifted(colour: Rgb): Rgb {
  const value = luminance(colour)
  if (!Number.isFinite(value) || value >= LUMINANCE_FLOOR) return colour
  const amount = Math.min(1, (LUMINANCE_FLOOR - value) / Math.max(1 - value, 0.0001))
  const mix = (channel: number) => channel + (1 - channel) * amount
  return { r: mix(colour.r), g: mix(colour.g), b: mix(colour.b) }
}

/** The wheel, solved once. */
export const WHEEL: Rgb[] = Array.from({ length: PALETTE_SIZE }, (_, index) =>
  levelled(WHEEL_HUE_OFFSET + (360 * index) / PALETTE_SIZE, WHEEL_SATURATION, WHEEL_TARGET_LUMINANCE),
)
const WHEEL_HUES: number[] = WHEEL.map(hueOf)

/** The nth colour on the wheel, as CSS. */
export function wheelColour(slot: number): string {
  return toCss(WHEEL[((slot % PALETTE_SIZE) + PALETTE_SIZE) % PALETTE_SIZE])
}

function coprime(first: number, second: number): boolean {
  let a = first
  let b = second
  while (b !== 0) {
    const t = b
    b = a % b
    a = t
  }
  return a === 1
}

/** How far apart two hues are, the short way round. */
export function separation(first: number, second: number): number {
  const raw = Math.abs(first - second) % 360
  return raw > 180 ? 360 - raw : raw
}

/**
 * How close the nearest pair of neighbouring rings would be, for one way of
 * dealing the wheel.
 *
 * **Two brand colours side by side do not count.** Claude Code's clay next to
 * MiniMax's red is what those two brands are; the mark cannot fix it and should
 * not be scored on it. Every pair with a dealt colour in it is ours.
 */
function worstNeighbour(brands: (Rgb | null)[], stride: number, rotation: number): number {
  const hues: { hue: number; isBrand: boolean }[] = []
  let index = 0
  for (const brand of brands) {
    if (brand) {
      hues.push({ hue: hueOf(lifted(brand)), isBrand: true })
    } else {
      hues.push({ hue: WHEEL_HUES[(rotation + stride * index) % PALETTE_SIZE], isBrand: false })
      index += 1
    }
  }

  let worst = 360
  for (let position = 0; position + 1 < hues.length; position++) {
    const first = hues[position]
    const second = hues[position + 1]
    if (first.isBrand && second.isBrand) continue
    worst = Math.min(worst, separation(first.hue, second.hue))
  }
  return worst
}

/**
 * Deal one colour per rail entry, in the order the rings are drawn.
 *
 * Some providers are monochrome by design and carry no colour at all. A row of
 * white bots is a rail you cannot read — the mark is the thing that says
 * *which* ring this is, and identical is the one thing it must not be — so
 * those are dealt a colour of Pulse's own.
 *
 * **Dealt across the rail, not fixed per provider.** A hash of the provider id
 * collided outright; a fixed palette in declaration order still sat a dealt
 * amber next to Claude Code's clay, because a hue can only be kept away from
 * its neighbours if you know who its neighbours are — and the rail shows
 * *enabled* accounts, so that is not knowable until it is drawn. Here it is:
 * every stride and every start is tried, and the one whose worst neighbouring
 * pair is furthest apart wins. Greedy choice ran out of colours near the end of
 * a long rail, which is exactly where it then had no choice but to put two
 * blues together.
 *
 * The cost is that adding or removing a provider can recolour the ones after
 * it. Nothing is stored against these colours, and a mark is not a reading, so
 * the trade is worth it.
 */
export function deal(brands: (string | null)[]): string[] {
  const parsed = brands.map((hex) => (hex ? parseCss(hex) : null))
  const dealtCount = parsed.filter((colour) => colour === null).length
  if (dealtCount === 0) return parsed.map((colour) => toCss(lifted(colour as Rgb)))

  let best: { score: number; stride: number; rotation: number } | null = null
  for (let stride = 1; stride < PALETTE_SIZE; stride++) {
    if (!coprime(stride, PALETTE_SIZE)) continue
    for (let rotation = 0; rotation < PALETTE_SIZE; rotation++) {
      const score = worstNeighbour(parsed, stride, rotation)
      if (best === null || score > best.score) best = { score, stride, rotation }
    }
  }
  const winner = best ?? { score: 0, stride: 3, rotation: 0 }

  let index = 0
  return parsed.map((colour) => {
    if (colour) return toCss(lifted(colour))
    const slot = (winner.rotation + winner.stride * index) % PALETTE_SIZE
    index += 1
    return toCss(WHEEL[slot])
  })
}

/**
 * Dark eyes on a light body, light eyes on a dark one.
 *
 * At 16pt contrast between the two is the entire face, which is why this is a
 * decision rather than a constant.
 */
export function eyesOn(body: Rgb | string = WHEEL[0]): string {
  const colour = typeof body === 'string' ? (parseCss(body) ?? WHEEL[0]) : body
  return luminance(colour) > 0.55 ? INK : PAPER
}

/** `rgb(r g b)` → channels. Anything else is treated as no colour. */
function parseCss(value: string): Rgb | null {
  const match = /^rgb\(\s*(\d+)\s+(\d+)\s+(\d+)\s*\)$/.exec(value.trim())
  if (!match) return null
  return { r: Number(match[1]) / 255, g: Number(match[2]) / 255, b: Number(match[3]) / 255 }
}

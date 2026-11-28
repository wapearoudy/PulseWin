import { memo, useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from 'react'
import type { AvatarOptions, MountedAvatar } from 'open-bot-avatar/dom'
import type { BotMarkMood } from '../botmark/mood'
import { avatarDefinition, type RobotFigure, type RobotMaterial } from './catalog'
import { AvatarPlayback, avatarState } from './playback'
import './robot-avatar.css'

export interface RobotAvatarProps {
  figure: RobotFigure
  material?: RobotMaterial
  colour: string
  mood: BotMarkMood
  accountId: string
  animated?: boolean
  active?: boolean
  finishedAt?: number
  resetAt?: number
  resetCelebrationAllowed?: boolean
  fallback?: ReactNode
  onMaterialFallback?: (fallback: boolean) => void
}

let supportsFluffy: boolean | undefined
function canDrawFluffy() {
  if (supportsFluffy !== undefined) return supportsFluffy
  try {
    const probe = document.createElement('canvas')
    const context = probe.getContext('webgl2', { alpha: true, antialias: true, preserveDrawingBuffer: true })
    supportsFluffy = context !== null && typeof Path2D !== 'undefined'
    context?.getExtension('WEBGL_lose_context')?.loseContext()
  } catch {
    supportsFluffy = false
  }
  return supportsFluffy
}

const reduceMotion = '(prefers-reduced-motion: reduce)'
function subscribeReducedMotion(changed: () => void) {
  const media = window.matchMedia(reduceMotion)
  media.addEventListener('change', changed)
  return () => media.removeEventListener('change', changed)
}
const prefersStill = () => window.matchMedia(reduceMotion).matches
const serverStill = () => true

function eyeColour(colour: string) {
  // Account colours are stored as hex; the existing automatic palette returns rgb().
  const hex = /^#([\da-f]{6})$/i.exec(colour)
  const rgb = /^rgb\(\s*(\d+)\s+(\d+)\s+(\d+)\s*\)$/.exec(colour)
  const channels = hex ? hex[1].match(/.{2}/g)!.map(channel => Number.parseInt(channel, 16)) : rgb?.slice(1).map(Number)
  if (!channels) return '#151515'
  const [r, g, b] = channels.map(channel => {
    const value = channel / 255
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.18 ? '#f0f0f0' : '#151515'
}

export const RobotAvatar = memo(function RobotAvatar({
  figure, material = 'flat', colour, mood, accountId, animated = true, active = true,
  finishedAt = 0, resetAt = 0, resetCelebrationAllowed = true, fallback, onMaterialFallback,
}: RobotAvatarProps) {
  const host = useRef<HTMLSpanElement>(null)
  const mounted = useRef<MountedAvatar | null>(null)
  const definition = useMemo(() => avatarDefinition(figure, accountId), [figure, accountId])
  const reduced = useSyncExternalStore(subscribeReducedMotion, prefersStill, serverStill)
  const still = reduced || !animated || !active
  const [ready, setReady] = useState(false)
  const [fluffy, setFluffy] = useState<boolean | undefined>()
  const [gesture, setGesture] = useState<'reset' | 'finish' | null>(null)
  const playback = useRef<{ account: string; player: AvatarPlayback } | null>(null)
  const actualMaterial = material === 'fluffy' && fluffy ? 'fluffy' : 'flat'

  useEffect(() => {
    if (material === 'fluffy') setFluffy(canDrawFluffy())
  }, [material])
  const materialFallback = material === 'fluffy' && fluffy === false
  useEffect(() => { onMaterialFallback?.(materialFallback) }, [materialFallback, onMaterialFallback])

  useEffect(() => {
    if (!playback.current || playback.current.account !== accountId) {
      playback.current = { account: accountId, player: new AvatarPlayback(finishedAt, resetAt) }
    }
    const now = Date.now()
    const current = playback.current.player.update(finishedAt, resetAt, resetCelebrationAllowed, mood, now)
    setGesture(current?.kind ?? null)
    if (current) {
      const timeout = window.setTimeout(() => setGesture(null), current.until - now)
      return () => window.clearTimeout(timeout)
    }
  }, [accountId, finishedAt, resetAt, resetCelebrationAllowed, mood])

  const state = avatarState[mood]
  const motion = gesture === 'reset' ? 'done' : state
  const options = useMemo<AvatarOptions | null>(() => definition ? {
    definition, material: actualMaterial, color: colour, eyeColor: eyeColour(colour),
    state: motion, mood: gesture === null ? 'neutral' : 'happy', still, reducedMotion: still,
    label: '',
  } : null, [definition, actualMaterial, colour, motion, gesture, still])
  const latest = useRef(options)
  latest.current = options

  useEffect(() => {
    const node = host.current
    if (!node) return
    let disposed = false
    setReady(false)
    const observer = new MutationObserver(() => {
      if (!disposed) setReady(node.firstElementChild !== null)
    })
    observer.observe(node, { childList: true })
    void import('open-bot-avatar/dom').then(({ mountAvatar }) => {
      if (!disposed && latest.current) mounted.current = mountAvatar(node, latest.current)
    }).catch(() => {
      // The provider mark is the usable state if rendering cannot initialize.
      if (!disposed) setReady(false)
    })
    return () => {
      disposed = true
      observer.disconnect()
      mounted.current?.destroy()
      mounted.current = null
    }
  }, [definition])
  useEffect(() => { if (options) mounted.current?.update(options) }, [options])

  return <span className="robot-avatar" data-figure={figure} data-material={actualMaterial}
    data-material-fallback={materialFallback} data-state={motion} data-gesture={gesture ?? undefined}
    data-motion={still ? 'still' : 'live'} data-ready={ready} aria-hidden="true">
    {!ready && fallback && <span className="robot-avatar__fallback">{fallback}</span>}
    <span className="robot-avatar__drawing" ref={host} />
  </span>
})

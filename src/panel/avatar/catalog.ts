import { examples, type AvatarDefinition } from 'open-bot-avatar'
import gallery from './gallery.json'

// Match Pulse's Automatic order, putting silhouettes that read at ring size first.
export const robotFigures = [
  ['mug', '杯子'], ['ghost', '幽灵'], ['cat', '猫'], ['lightbulb', '灯泡'],
  ['chat', '对话气泡'], ['cloud', '云朵'], ['egg', '鸡蛋'], ['controller', '手柄'],
  ['bunny', '兔子'], ['star', '星星'], ['rocket', '火箭'], ['book', '书本'],
  ['camera', '相机'], ['shield', '盾牌'], ['sprout', '幼苗'], ['brain', '大脑'],
  ['bull', '公牛'], ['palette', '调色板'], ['magnifier', '放大镜'], ['witch-hat', '巫师帽'],
  ['paint-brush', '画笔'], ['barbell', '杠铃'], ['watering-can', '洒水壶'],
] as const

export type RobotFigure = typeof robotFigures[number][0]
export type RobotMaterial = 'flat' | 'fluffy'

const definitions = new Map<string, AvatarDefinition>(examples.map(figure => [figure.id, figure]))
for (const figure of gallery) {
  if (figure.version === 1) definitions.set(figure.id, { ...figure, version: 1 })
}
const ids = robotFigures.map(([id]) => id)
const known: ReadonlySet<string> = new Set(ids)

function isRobotFigure(id: string | null | undefined): id is RobotFigure {
  return id != null && known.has(id)
}

export function robotFigureAt(id: string | null | undefined, index = 0): RobotFigure {
  return isRobotFigure(id) ? id : ids[((index % ids.length) + ids.length) % ids.length]
}

export function spreadRobotFigures(chosen: readonly (string | null | undefined)[]): RobotFigure[] {
  const resolved: RobotFigure[] = []
  for (const [index, id] of chosen.entries()) {
    if (isRobotFigure(id)) {
      resolved.push(id)
      continue
    }
    const previous = resolved[index - 1]
    const next = chosen[index + 1]
    for (let offset = 0; offset < ids.length; offset++) {
      const candidate = robotFigureAt(null, index + offset)
      if (candidate !== previous && candidate !== next) {
        resolved.push(candidate)
        break
      }
    }
  }
  return resolved
}

export function avatarDefinition(figure: RobotFigure, accountId: string): AvatarDefinition | undefined {
  const definition = definitions.get(figure)
  if (!definition) return
  // The renderer uses the definition id to offset its motion clock. Give the
  // same shape a stable phase per account, even if two people pick it manually.
  let seed = 7
  for (const character of accountId) seed = (Math.imul(seed, 31) + character.codePointAt(0)!) >>> 0
  return { ...definition, id: `${figure}-${seed.toString(36)}` }
}

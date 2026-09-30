import type { Tool } from './types'

/** The names `Icon` draws. */
export type IconName = Tool | 'undo' | 'redo'

export interface ToolInfo {
  id: Tool
  label: string
  key: string
  code: string
  hint: string
}

/** The tools on the view's edge, in order, with their keys. */
export const TOOLS: ToolInfo[] = [
  { id: 'select', label: 'Select', key: 'V', code: 'KeyV', hint: 'click a node, a link or an entity' },
  {
    id: 'link',
    label: 'Link',
    key: 'L',
    code: 'KeyL',
    hint: 'click the node a link starts at, then the one it goes to; the next link starts there; Esc stops',
  },
  { id: 'unlink', label: 'Unlink', key: 'U', code: 'KeyU', hint: 'click a link to take it out' },
  {
    id: 'node',
    label: 'Node',
    key: 'N',
    code: 'KeyN',
    hint: 'click the floor where a node is missing: it is linked with the nodes around that check out',
  },
  {
    id: 'move',
    label: 'Move',
    key: 'M',
    code: 'KeyM',
    hint: 'drag a node to where it should stand: its links are checked again there; Esc while dragging leaves it',
  },
  { id: 'forbid', label: 'Forbid', key: 'X', code: 'KeyX', hint: 'click where bots must never plan through' },
  { id: 'place', label: 'Place', key: 'P', code: 'KeyP', hint: 'click where the named place is' },
  {
    id: 'route',
    label: 'Route',
    key: 'R',
    code: 'KeyR',
    hint: 'click where a bot starts, then where it goes: the way it plans with the changes',
  },
]

export function toolInfo(id: Tool): ToolInfo {
  return TOOLS.find((t) => t.id === id) ?? TOOLS[0]
}

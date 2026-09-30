import { useNav } from './stores/nav'
import { TOOLS } from './tools'

/** Ctrl lowers the camera, so shortcuts are ⌘ on a Mac; elsewhere Ctrl pressed just before the key. */
export const MAC = navigator.userAgent.includes('Mac')
export const MOD = MAC ? '⌘' : 'Ctrl+'
/** Ctrl held longer than this before the key is flying down, not a shortcut (ms). */
const CTRL_SHORTCUT = 600

/** The editor's keys: tools, undo and redo, save, Esc and Delete. Returns what takes them away. */
export function installShortcuts(): () => void {
  const nav = useNav()
  let ctrlAt = 0
  const shortcut = (e: KeyboardEvent) => (MAC ? e.metaKey : e.ctrlKey && performance.now() - ctrlAt < CTRL_SHORTCUT)

  const onKey = (e: KeyboardEvent) => {
    if ((e.code === 'ControlLeft' || e.code === 'ControlRight') && !e.repeat) {
      ctrlAt = performance.now()
    }
    const t = e.target
    if (t instanceof HTMLInputElement || t instanceof HTMLSelectElement || t instanceof HTMLTextAreaElement) {
      return
    }
    if (shortcut(e)) {
      if (e.code === 'KeyZ') {
        e.preventDefault()
        if (e.shiftKey) nav.redo()
        else nav.undo()
      } else if (e.code === 'KeyY') {
        e.preventDefault()
        nav.redo()
      } else if (e.code === 'KeyS') {
        e.preventDefault()
        void nav.save()
      }
      return
    }
    if (e.metaKey || e.ctrlKey || e.altKey) return
    if (e.code === 'Escape') {
      nav.cancel()
      nav.clearSelection()
    } else if (e.code === 'Delete' || e.code === 'Backspace') {
      if (nav.selPatch !== null) nav.removePatch(nav.selPatch)
      else if (nav.selLink) nav.unlink(nav.selLink)
    } else {
      const tool = TOOLS.find((x) => x.code === e.code)
      if (tool) nav.setTool(tool.id)
    }
  }
  window.addEventListener('keydown', onKey)
  return () => window.removeEventListener('keydown', onKey)
}

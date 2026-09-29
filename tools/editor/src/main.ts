import { createPinia } from 'pinia'
import { createApp } from 'vue'

import App from './App.vue'
import './style.css'

createApp(App).use(createPinia()).mount('#app')

// Buttons, lists, sliders and checkboxes let go of the keyboard once used with the mouse: arrows, Space and the rest
// fly the camera instead of changing them (a list with the focus would switch maps on an arrow).
let byPointer = false
document.addEventListener('pointerdown', () => (byPointer = true), true)
document.addEventListener('keydown', () => (byPointer = false), true)
const letGo = (e: Event) => {
  if (!byPointer || !(e.target instanceof HTMLElement)) return
  const t = e.target
  const held = t.closest<HTMLElement>('button, summary')
  if (held) {
    held.blur()
  } else if (t instanceof HTMLSelectElement || (t instanceof HTMLInputElement && ['checkbox', 'range', 'radio'].includes(t.type))) {
    t.blur()
  }
}
document.addEventListener('click', letGo)
document.addEventListener('change', letGo)

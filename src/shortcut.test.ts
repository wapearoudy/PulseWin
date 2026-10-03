import { describe, expect, it } from 'vitest'
import { captureShortcut, shortcutLabel, heldModifiers, type ShortcutKey } from './shortcut'
const key=(code:string,patch:Partial<ShortcutKey>={}):ShortcutKey=>({code,ctrlKey:false,altKey:false,metaKey:false,shiftKey:false,...patch})
describe('local shortcut recorder',()=>{
  it('requires Ctrl, Alt or Win and preserves physical codes',()=>{
    expect(captureShortcut(key('KeyP'))).toEqual({kind:'invalid'})
    expect(captureShortcut(key('KeyP',{shiftKey:true}))).toEqual({kind:'invalid'})
    expect(captureShortcut(key('KeyP',{ctrlKey:true,altKey:true,metaKey:true,shiftKey:true}))).toEqual({kind:'shortcut',value:'Control+Alt+Super+Shift+KeyP'})
    expect(captureShortcut(key('F8',{altKey:true}))).toEqual({kind:'shortcut',value:'Alt+F8'})
  })
  it('cancels and clears only bare escape/delete, preserving modified combinations',()=>{
    expect(captureShortcut(key('Escape'))).toEqual({kind:'cancel'})
    for(const code of ['Backspace','Delete'])expect(captureShortcut(key(code))).toEqual({kind:'clear'})
    expect(captureShortcut(key('Backspace',{ctrlKey:true}))).toEqual({kind:'shortcut',value:'Control+Backspace'})
    expect(captureShortcut(key('Escape',{altKey:true}))).toEqual({kind:'shortcut',value:'Alt+Escape'})
  })
  it('shows modifiers without storing half a combination or autorepeat',()=>{
    expect(captureShortcut(key('ControlLeft',{ctrlKey:true}))).toEqual({kind:'held',label:'Ctrl'})
    expect(captureShortcut(key('KeyP',{ctrlKey:true,repeat:true}))).toEqual({kind:'held',label:'Ctrl'})
    expect(captureShortcut(key('KeyP',{ctrlKey:true,isComposing:true}))).toEqual({kind:'held',label:'Ctrl'})
    expect(captureShortcut(key('Unidentified',{ctrlKey:true}))).toEqual({kind:'invalid'})
    expect(heldModifiers(key('KeyP',{ctrlKey:true,shiftKey:true}))).toBe('Ctrl + Shift')
  })
  it('displays saved codes as recognizable Windows combinations',()=>{
    expect(shortcutLabel(null)).toBe('未设置');expect(shortcutLabel('Control+Alt+KeyP')).toBe('Ctrl + Alt + P')
    expect(shortcutLabel('Super+Shift+Digit7')).toBe('Win + Shift + 7');expect(shortcutLabel('Alt+ArrowLeft')).toBe('Alt + ←')
    expect(shortcutLabel('Control+Numpad3')).toBe('Ctrl + 数字 3')
  })
})

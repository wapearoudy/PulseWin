export interface ShortcutKey {
  code:string;ctrlKey:boolean;altKey:boolean;metaKey:boolean;shiftKey:boolean
  repeat?:boolean;isComposing?:boolean
}
export type CapturedShortcut = {kind:'held';label:string}|{kind:'cancel'}|{kind:'clear'}|{kind:'invalid'}|{kind:'shortcut';value:string}
const modifiers=new Set(['ControlLeft','ControlRight','AltLeft','AltRight','MetaLeft','MetaRight','ShiftLeft','ShiftRight','OSLeft','OSRight'])
export function heldModifiers(event:Pick<ShortcutKey,'ctrlKey'|'altKey'|'metaKey'|'shiftKey'>):string {
  return [event.ctrlKey?'Ctrl':null,event.altKey?'Alt':null,event.metaKey?'Win':null,event.shiftKey?'Shift':null].filter(Boolean).join(' + ')
}
/** Only the local recorder calls this; its DOM event is swallowed there.
 * Physical key codes keep the saved combination independent of keyboard layout. */
export function captureShortcut(event:ShortcutKey):CapturedShortcut {
  if(event.isComposing||event.repeat||modifiers.has(event.code))return {kind:'held',label:heldModifiers(event)}
  const bare=!event.ctrlKey&&!event.altKey&&!event.metaKey&&!event.shiftKey
  if(bare&&event.code==='Escape')return {kind:'cancel'}
  if(bare&&(event.code==='Backspace'||event.code==='Delete'))return {kind:'clear'}
  if(!event.ctrlKey&&!event.altKey&&!event.metaKey||!event.code||['Unidentified','Process','Fn'].includes(event.code))return {kind:'invalid'}
  const held=[event.ctrlKey?'Control':null,event.altKey?'Alt':null,event.metaKey?'Super':null,event.shiftKey?'Shift':null].filter(Boolean)
  return {kind:'shortcut',value:[...held,event.code].join('+')}
}
const keyNames:Record<string,string>={Space:'空格',Enter:'Enter',Escape:'Esc',Backspace:'Backspace',Delete:'Delete',ArrowLeft:'←',ArrowRight:'→',ArrowUp:'↑',ArrowDown:'↓',Comma:',',Period:'.',Slash:'/',Backslash:'\\',Semicolon:';',Quote:"'",BracketLeft:'[',BracketRight:']',Minus:'−',Equal:'=',Backquote:'`',NumpadAdd:'数字 +',NumpadSubtract:'数字 −',NumpadMultiply:'数字 ×',NumpadDivide:'数字 ÷',NumpadDecimal:'数字 .',NumpadEnter:'数字 Enter'}
export function shortcutLabel(value:string|null|undefined):string {
  if(!value)return '未设置'
  return value.split('+').map(key=>key==='Control'?'Ctrl':key==='Super'?'Win':keyNames[key]??key.replace(/^Key(?=[A-Z]$)/,'').replace(/^Digit(?=\d$)/,'').replace(/^Numpad(?=\d$)/,'数字 ')).join(' + ')
}

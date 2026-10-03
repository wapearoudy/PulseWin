import { test, expect, type Page } from '@playwright/test'

test.beforeEach(async({page})=>{
  await page.addInitScript(()=>{
    const w=window as any;let sequence=0
    const callbacks:Record<number,Function>={},listeners:Record<number,any>={}
    w.updateState={currentVersion:'0.1.4',settings:{source:'E:/updates',automatic:true},phase:'idle',version:null,notes:null,downloaded:0,total:null,checkedAt:null,error:null}
    w.updateCommands=[];w.updateAvailable=false;w.updateSignatureFailure=false
    w.updateEmit=(patch:any)=>{Object.assign(w.updateState,patch);Object.values(listeners).filter(x=>x.event==='update-status').forEach(x=>callbacks[x.handler]?.({event:'update-status',payload:structuredClone(w.updateState)}))}
    w.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener:(_:string,id:number)=>delete listeners[id]}
    w.__TAURI_INTERNALS__={transformCallback:(fn:Function)=>{callbacks[++sequence]=fn;return sequence},invoke:async(cmd:string,args:any={})=>{
      w.updateCommands.push({cmd,args})
      if(cmd==='plugin:event|listen'){listeners[++sequence]=args;return sequence}
      if(cmd==='plugin:event|unlisten'){delete listeners[args.eventId];return}
      if(cmd==='get_preferences')return {enabledProviders:['claude-code'],providerOrder:['claude-code'],accountAppearance:{},pinnedWindows:{}}
      if(cmd==='provider_settings')return [{id:'claude-code',name:'Claude Code',configured:false,stored:false,hints:[]}]
      if(cmd==='get_snapshot')return {providers:[],fetchedAt:new Date().toISOString()}
      if(cmd==='panel_placement')return {dockEdge:'right',railPosition:null}
      if(cmd==='get_application_settings')return {openSettingsShortcut:null,togglePanelShortcut:null,unavailable:{},autostartEnabled:false,autostartError:null}
      if(cmd==='get_update_status')return structuredClone(w.updateState)
      if(cmd==='check_app_update'){
        w.updateEmit({phase:'checking',error:null});await new Promise(r=>setTimeout(r,50))
        w.updateEmit({phase:w.updateAvailable?'available':'current',version:w.updateAvailable?'0.1.5':null,notes:w.updateAvailable?'新增功能与修复':null,checkedAt:new Date().toISOString()});return structuredClone(w.updateState)
      }
      if(cmd==='save_update_settings'){
        if(args.settings.source.startsWith('http://'))throw new Error('不支持普通 HTTP')
        w.updateEmit({settings:args.settings,phase:'idle',version:null,error:null});return structuredClone(w.updateState)
      }
      if(cmd==='install_app_update'){
        w.updateEmit({phase:'downloading',downloaded:512,total:1024});await new Promise(r=>setTimeout(r,250))
        if(w.updateSignatureFailure){w.updateEmit({phase:'error',error:'更新下载或签名校验失败，当前版本未改变。请重新检查后重试。'});throw new Error(w.updateState.error)}
        w.updateEmit({phase:'installing',downloaded:1024,total:1024});return
      }
    }}
  })
  await page.goto('/?view=settings&account=general')
  await expect(page.getByRole('button',{name:'检查更新',exact:true})).toBeEnabled()
})
const updater=(page:Page)=>page.getByRole('region',{name:'软件更新'})
test('automatic checks and manual no-update checks never install or change accounts',async({page})=>{
  await expect(page.getByRole('switch',{name:'自动检查更新'})).toBeChecked()
  await page.getByRole('button',{name:'检查更新',exact:true}).click()
  await expect(updater(page).getByRole('status')).toHaveText('当前已是更新来源中的最新版本')
  expect(await page.evaluate(()=>(window as any).updateCommands.filter((x:any)=>['install_app_update','save_preferences','refresh_provider'].includes(x.cmd)))).toEqual([])
})
test('available version requires an install action and reports progress then restart',async({page})=>{
  await page.evaluate(()=>(window as any).updateAvailable=true)
  await page.getByRole('button',{name:'检查更新',exact:true}).click()
  await expect(page.getByRole('button',{name:'更新并重启'})).toBeVisible()
  await expect(updater(page)).toContainText('新增功能与修复')
  await page.getByRole('button',{name:'更新并重启'}).click()
  await expect(page.getByRole('progressbar',{name:'更新下载进度'})).toBeVisible()
  await expect(updater(page).getByRole('status')).toContainText('自动重启')
  expect(await page.evaluate(()=>(window as any).updateCommands.filter((x:any)=>x.cmd==='install_app_update').map((x:any)=>x.args))).toEqual([{version:'0.1.5'}])
})
test('signature failure keeps the old version and supports a fresh retry',async({page})=>{
  await page.evaluate(()=>{(window as any).updateAvailable=true;(window as any).updateSignatureFailure=true})
  await page.getByRole('button',{name:'检查更新',exact:true}).click();await page.getByRole('button',{name:'更新并重启'}).click()
  await expect(updater(page).getByRole('alert')).toContainText('签名校验失败')
  await expect(updater(page)).toContainText('PulseWin 0.1.4')
  await page.evaluate(()=>(window as any).updateSignatureFailure=false)
  await page.getByRole('button',{name:'检查更新',exact:true}).click();await expect(page.getByRole('button',{name:'更新并重启'})).toBeEnabled()
})
test('source edits must be saved before checking and support GitHub Releases',async({page})=>{
  await page.getByRole('textbox',{name:'更新来源'}).fill('https://github.com/example/pulsewin/releases/latest/download/latest.json')
  await expect(page.getByRole('button',{name:'检查更新',exact:true})).toBeDisabled()
  await page.getByRole('button',{name:'保存来源'}).click()
  await expect(page.getByRole('button',{name:'检查更新',exact:true})).toBeEnabled()
  expect(await page.evaluate(()=>(window as any).updateState.settings.source)).toContain('/releases/latest/download/latest.json')
  await page.getByRole('switch',{name:'自动检查更新'}).uncheck()
  expect(await page.evaluate(()=>(window as any).updateState.settings.automatic)).toBe(false)
})
test('invalid source stays editable and does not replace the saved channel',async({page})=>{
  await page.getByRole('textbox',{name:'更新来源'}).fill('http://example.com/latest.json');await page.getByRole('button',{name:'保存来源'}).click()
  await expect(updater(page).getByRole('alert')).toContainText('不支持普通 HTTP')
  expect(await page.evaluate(()=>(window as any).updateState.settings.source)).toBe('E:/updates')
})

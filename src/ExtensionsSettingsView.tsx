import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

export interface ExtensionView {
  id: string
  name: string
  directory: string
  executable: string
  timeoutSeconds: number
  accountId: string
}

export interface ExtensionProblem { folder: string; message: string }
export interface ExtensionsScan {
  folder: string
  extensions: ExtensionView[]
  problems: ExtensionProblem[]
}

const GUIDE = 'https://github.com/qunqin24/Pulse/blob/main/Docs/extensions.md'

/// `~` for the home folder: the full path is mostly a user name.
export function displayPath(path: string): string {
  return path.replace(/^([A-Za-z]:\\Users\\[^\\]+|\/Users\/[^/]+|\/home\/[^/]+)/, '~')
}

/// The extensions pane: where they live, which were found, and which folders
/// could not be used and why.
///
/// **A scan, not a run.** Opening this pane starts no program; an extension is
/// asked for a reading only once it is switched on and a refresh comes round.
export function ExtensionsSettingsView({ enabled }: { enabled: string[] }) {
  const [scan, setScan] = useState<ExtensionsScan | null>(null)
  const [error, setError] = useState<string | null>(null)
  const rescan = useCallback(() => {
    setError(null)
    void invoke<ExtensionsScan>('extensions_settings').then(setScan).catch(e => setError(String(e)))
  }, [])
  /// Switching one on writes the account, so it goes through Rust rather than a
  /// preferences patch: only Rust can check the folder still holds a program.
  const switchOn = (account: string, on: boolean) => {
    setError(null)
    void invoke<void>('set_extension_enabled', { account, enabled: on })
      .then(rescan)
      .catch(e => setError(String(e)))
  }
  // Somebody opening this pane has most likely just put something in the
  // folder. Reading it costs a directory listing and a few small files.
  useEffect(rescan, [rescan])
  const openFolder = () => invoke<void>('open_extensions_folder').catch(e => setError(String(e)))
  return <>
    <h1>用户扩展</h1>
    <p className="page-description">扩展是你自己添加的程序，用来报告一个账号的用量。Pulse 把它打印的内容画成圆环，只在你开启后才运行，并且不给它任何凭据。</p>
    <section className="group-wrap"><h2>文件夹</h2><div className="settings-group">
      <div className="setting-row"><div><strong>扩展文件夹</strong><small>{scan ? displayPath(scan.folder) : '正在读取…'}</small></div>
        <div className="setting-control"><button disabled={!scan} onClick={openFolder}>在文件夹中显示</button></div></div>
      <div className="setting-row"><div><strong>重新读取</strong><small>Pulse 在启动时读取这个文件夹。添加或修改扩展后，请重新读取。</small></div>
        <div className="setting-control"><button onClick={rescan}>重新读取</button></div></div>
      <div className="setting-row"><div><strong>如何编写扩展</strong><small>清单格式、输出格式与完整示例。</small></div>
        <div className="setting-control"><a href={GUIDE} target="_blank" rel="noreferrer">打开说明</a></div></div>
    </div></section>
    {error && <div role="alert" className="settings-alert">{error}</div>}
    <section className="group-wrap"><h2>已找到</h2><div className="settings-group">
      {!scan ? <p className="group-copy" role="status">正在读取扩展文件夹…</p>
        : !scan.extensions.length ? <p className="group-copy">还没有扩展。每个扩展是一个独立文件夹，里面有 pulse-extension.json 和它的程序。</p>
        : scan.extensions.map(found => <div className="setting-row" key={found.id}>
            <div><strong>{found.name}</strong><small>{enabled.includes(found.accountId) ? '显示在面板' : '已关闭'} · 时限 {Math.round(found.timeoutSeconds)} 秒</small></div>
            <div className="setting-control"><input type="checkbox" role="switch" aria-label={`启用 ${found.name}`} checked={enabled.includes(found.accountId)} onChange={e => switchOn(found.accountId, e.target.checked)}/></div>
          </div>)}
    </div></section>
    {scan && scan.problems.length > 0 && <section className="group-wrap"><h2>无法使用</h2><div className="settings-group">
      {scan.problems.map(problem => <div className="setting-row" key={problem.folder}>
        <div><strong>{problem.folder}</strong><small>{problem.message}</small></div>
      </div>)}
    </div></section>}
    <p className="group-copy">扩展在每次刷新时最多运行一个时限，超时即停止；输出必须是一个 JSON 对象，无法解析时只报告一条说明，不会画出猜测的数字。</p>
  </>
}

/// What an extension's own account pane says about the program behind it:
/// which file runs, from where, and for how long at most.
///
/// Read from the catalogue rather than carried through the account list: the
/// account says only that an id is switched on, and the program may have been
/// changed or removed since.
export function ExtensionProgramGroup({ accountId }: { accountId: string }) {
  const [found, setFound] = useState<ExtensionView | null>(null)
  const [missing, setMissing] = useState(false)
  useEffect(() => {
    let live = true
    void invoke<ExtensionsScan>('extensions_settings')
      .then(scan => { if (!live) return
        const match = scan.extensions.find(e => e.accountId === accountId)
        setFound(match ?? null); setMissing(!match) })
      .catch(() => { if (live) setMissing(true) })
    return () => { live = false }
  }, [accountId])
  if (missing) return <section className="group-wrap"><h2>扩展</h2><div className="settings-group">
    <p className="group-copy" role="alert">这个扩展已不在扩展文件夹中。重新放入程序后请到「用户扩展」重新读取。</p>
  </div></section>
  if (!found) return null
  return <section className="group-wrap"><h2>扩展</h2><div className="settings-group">
    <div className="setting-row"><div><strong>程序</strong><small>{displayPath(found.executable)}</small></div></div>
    <div className="setting-row"><div><strong>时间限制</strong><small>超过这个时间仍未回答就会被停止。</small></div>
      <div className="setting-control"><span>{Math.round(found.timeoutSeconds)} 秒</span></div></div>
    <div className="setting-row"><div><strong>标识</strong><small>来自它的 pulse-extension.json。Pulse 用这个标识保存这个扩展的设置。</small></div>
      <div className="setting-control"><code>{found.id}</code></div></div>
  </div></section>
}

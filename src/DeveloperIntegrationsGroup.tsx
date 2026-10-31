import { useState } from 'react'

/** Navigation-only links and read-only CLI helpers for external integrations. */
export interface IntegrationAccount {
  id: string
  name: string
}

const STATUSLINE_SCRIPT = `# PulseWin status line for PowerShell
# Keeps the tray app running; reads the saved cache without fetching anything.
$pulse = Join-Path $env:LOCALAPPDATA 'Programs\\PulseWin\\PulseWin.exe'
if (-not (Test-Path $pulse)) { $pulse = 'pulsewin.exe' }
& $pulse --statusline
`
const SHELL_SCRIPT = `#!/bin/sh
# PulseWin status line for Git Bash, WSL or any shell on the Windows PATH.
# Keeps the tray app running; reads the saved cache without fetching anything.
exec pulsewin.exe --statusline
`
const README = `PulseWin developer kit
======================

pulsewin-status.ps1  PowerShell status-line helper (runs pulsewin.exe --statusline).
pulsewin-status.sh   POSIX shell helper for Git Bash/WSL.
pulsewin.exe --json  Full saved readings as JSON.
pulsewin.exe --recap [YYYY-MM-DD | week-YYYY-MM-DD | YYYY-MM | YYYY]
                     One day, week, month or year of saved Token figures.

All commands are read-only: they never fetch providers or open credentials.
Keep PulseWin running so the saved cache stays current. Every reading carries
its age; an old reading is not a live check.

Raycast and Sketchybar are macOS-only integrations from the original app and
have no Windows equivalent here; use the same commands from any Windows launcher.
`

export function DeveloperIntegrationsGroup({ accounts }: { accounts: IntegrationAccount[] }) {
  const [selected, setSelected] = useState(accounts[0]?.id ?? '')
  const [message, setMessage] = useState<string | null>(null)
  const account = accounts.find(item => item.id === selected) ?? accounts[0]
  const copy = (text: string) => {
    void navigator.clipboard.writeText(text).then(
      () => setMessage('已复制'),
      () => setMessage('复制失败，请手动选择文本。'),
    )
  }
  const saveScript = () => {
    const url = URL.createObjectURL(new Blob([STATUSLINE_SCRIPT], { type: 'text/plain' }))
    const link = document.createElement('a')
    link.href = url
    link.download = 'pulsewin-status.ps1'
    link.click()
    URL.revokeObjectURL(url)
    setMessage('状态栏脚本已保存')
  }
  const saveFile = (name: string, text: string) => {
    const url = URL.createObjectURL(new Blob([text], { type: 'text/plain' }))
    const link = document.createElement('a')
    link.href = url
    link.download = name
    link.click()
    URL.revokeObjectURL(url)
  }
  const exportKit = () => {
    saveFile('pulsewin-status.ps1', STATUSLINE_SCRIPT)
    saveFile('pulsewin-status.sh', SHELL_SCRIPT)
    saveFile('README.txt', README)
    setMessage('开发者套件已导出（3 个文件）')
  }
  return (
    <>
      <h1>开发者集成</h1>
      <p className="page-description">
        读取 PulseWin 已保存的读数，不发起网络请求，也不打开凭据。适合状态栏、脚本和其他桌面工具。
      </p>
      <section className="group-wrap" aria-label="读取缓存的用量">
        <h2>读取缓存的用量</h2>
        <div className="settings-group">
          <div className="setting-row">
            <div>
              <strong>PulseWin --json</strong>
              <small>打印已保存的读数，而不是服务商的原始回复；不刷新、不读取凭据。</small>
            </div>
            <div className="setting-control">
              <button onClick={() => copy('pulsewin.exe --json')}>复制命令</button>
            </div>
          </div>
          <div className="setting-row">
            <div>
              <strong>PulseWin --statusline</strong>
              <small>一行纯文本，按账号顺序列出每个账号最紧要的百分比。</small>
            </div>
            <div className="setting-control">
              <button onClick={() => copy('pulsewin.exe --statusline')}>复制命令</button>
            </div>
          </div>
          <div className="setting-row">
            <div>
              <strong>状态栏脚本</strong>
              <small>保存一个 PowerShell 脚本，把 --statusline 的输出接到自己的状态栏。</small>
            </div>
            <div className="setting-control">
              <button onClick={saveScript}>保存脚本</button>
            </div>
          </div>
          <div className="setting-row">
            <div>
              <strong>开发者套件</strong>
              <small>导出 PowerShell、POSIX shell 与说明文件；Raycast/Sketchybar 为原版 macOS 集成，Windows 无对应程序。</small>
            </div>
            <div className="setting-control">
              <button onClick={exportKit}>导出文件</button>
            </div>
          </div>
        </div>
      </section>
      <section className="group-wrap" aria-label="打开账号">
        <h2>打开账号</h2>
        <div className="settings-group">
          <div className="setting-row">
            <div>
              <strong>账号</strong>
              <small>链接只负责打开这个账号的设置页，不能修改设置、运行命令或开始登录。</small>
            </div>
            <div className="setting-control">
              <select aria-label="集成账号" value={selected} onChange={event => { setSelected(event.target.value); setMessage(null) }}>
                {accounts.length === 0 && <option value="">尚未添加账号</option>}
                {accounts.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
              </select>
            </div>
          </div>
          <div className="setting-row">
            <div>
              <strong>账号链接</strong>
              <small>交给已注册 pulsewin:// 协议的安装版即可打开该账号。</small>
            </div>
            <div className="setting-control">
              <button disabled={!account} onClick={() => account && copy(`pulsewin://account/${account.id}`)}>复制链接</button>
            </div>
          </div>
          <div className="setting-row">
            <div>
              <strong>从终端打开</strong>
              <small>在“运行”或 PowerShell 中启动同一链接。</small>
            </div>
            <div className="setting-control">
              <button disabled={!account} onClick={() => account && copy(`start "" "pulsewin://account/${account.id}"`)}>复制命令</button>
            </div>
          </div>
        </div>
      </section>
      <p className="group-copy">保持 PulseWin 运行以更新缓存。每条读数都带有年龄；旧读数不是一次实时检查。</p>
      {message && <p className="group-copy" role="status">{message}</p>}
    </>
  )
}

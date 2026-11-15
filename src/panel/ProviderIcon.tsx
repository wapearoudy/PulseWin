import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

// Original Pulse SVG resources, used as monochrome provider glyphs.
const icons = import.meta.glob('./icons/*.svg', {eager:true,query:'?url',import:'default'}) as Record<string,string>
const aliases:Record<string,string> = {'claude-code':'claude',codex:'openai',copilot:'github','grok-bot':'grok','opencode-go':'opencode','kimi-code':'kimi','glm-coding':'zai','minimax-cn':'minimax','hugging-face':'huggingface','command-code':'commandcode','new-api':'newapi','xiaomi-coding-plan':'xiaomimimo','openai-api':'openai','xai-api':'xai','ollama-cloud':'ollama','alibaba-coding-plan':'alibabacloud'}

/// An extension's own account id, exactly as Rust builds it. The icon that goes
/// with it is a PNG Rust has already checked, drawn in its own colour: a
/// program's artwork, not a mask to be tinted with `currentColor`.
const EXTENSION_ACCOUNT = 'extension--account-'

const extensionIcons = new Map<string, string>()
let scanning: Promise<void> | null = null

/// Remember what a scan found, keeping an icon that is still there and dropping
/// one that is gone. Exported so a test can draw the coloured path without a
/// running app behind it.
export function rememberExtensionIcons(found: { accountId: string; icon?: string | null }[]) {
  for (const extension of found) {
    if (extension.icon) extensionIcons.set(extension.accountId, extension.icon)
    else extensionIcons.delete(extension.accountId)
  }
}

/// One scan per webview: the ring, the card, the settings list and the tray all
/// ask about the same accounts.
function scanOnce(): Promise<void> {
  scanning ??= invoke<{ extensions?: { accountId: string; icon?: string | null }[] }>('extensions_settings')
    .then(scan => rememberExtensionIcons(scan?.extensions ?? []))
    // No scan only means no artwork: every extension then draws the default
    // glyph, which is also what an extension with no icon of its own does.
    .catch(() => {})
  return scanning
}

/// The icon behind an extension's account, once it has arrived. Built-ins never
/// ask for one.
function useExtensionIcon(provider: string): string | undefined {
  const wanted = provider.startsWith(EXTENSION_ACCOUNT) ? provider : ''
  const [icon, setIcon] = useState<string | undefined>(() => extensionIcons.get(wanted))
  useEffect(() => {
    if (!wanted) return
    const known = extensionIcons.get(wanted)
    if (known) { setIcon(known); return }
    let live = true
    void scanOnce().then(() => { if (live) setIcon(extensionIcons.get(wanted)) })
    return () => { live = false }
  }, [wanted])
  return wanted ? icon : undefined
}

export function ProviderIcon({provider}:{provider:string}) {
  const original = useExtensionIcon(provider)
  if (original) {
    return <span className="provider-glyph" data-provider-icon={provider} data-original-colour="true" style={{display:'block',width:'100%',height:'100%',backgroundImage:`url("${original}")`,backgroundPosition:'center',backgroundSize:'contain',backgroundRepeat:'no-repeat'}} />
  }
  const base=provider.split('--account-')[0]
  const name=aliases[base]??base.replace(/-/g,'')
  const url=icons[`./icons/${name}.svg`]??icons['./icons/extension.svg']
  return <span className="provider-glyph" data-provider-icon={provider} style={{display:'block',width:'100%',height:'100%',backgroundColor:'currentColor',mask:`url("${url}") center / contain no-repeat`,WebkitMask:`url("${url}") center / contain no-repeat`}} />
}

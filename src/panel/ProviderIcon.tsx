// Original Pulse SVG resources, used as monochrome provider glyphs.
const icons = import.meta.glob('./icons/*.svg', {eager:true,query:'?url',import:'default'}) as Record<string,string>
const aliases:Record<string,string> = {'claude-code':'claude',codex:'openai',copilot:'github','grok-bot':'grok','opencode-go':'opencode','kimi-code':'kimi','glm-coding':'zai','minimax-cn':'minimax','hugging-face':'huggingface','command-code':'commandcode','new-api':'newapi','xiaomi-coding-plan':'xiaomimimo','openai-api':'openai','xai-api':'xai','ollama-cloud':'ollama','alibaba-coding-plan':'alibabacloud'}
export function ProviderIcon({provider}:{provider:string}) {
  const base=provider.split('--account-')[0]
  const name=aliases[base]??base.replace(/-/g,'')
  const url=icons[`./icons/${name}.svg`]??icons['./icons/extension.svg']
  return <span className="provider-glyph" data-provider-icon={provider} style={{display:'block',width:'100%',height:'100%',backgroundColor:'currentColor',mask:`url("${url}") center / contain no-repeat`,WebkitMask:`url("${url}") center / contain no-repeat`}} />
}

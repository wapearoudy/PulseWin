import {useCallback,useEffect,useState,useRef} from 'react'
import {invoke} from '@tauri-apps/api/core'
interface Connection{connected:boolean}
interface Day{date:string;input:number;output:number;cacheRead:number;tokens:string;cost:number|null;models:Record<string,number>}
interface History{days:Day[];currency:string|null;checkedAt:string;notes:string[]}
export function DeepSeekConsoleGroup({enabled,onChanged}:{enabled:boolean;onChanged:()=>void}){
 const [connection,setConnection]=useState<Connection|null>(null),[history,setHistory]=useState<History|null>(null),[error,setError]=useState<string|null>(null),[busy,setBusy]=useState(false),[opened,setOpened]=useState(false),[all,setAll]=useState(false)
 const revision=useRef(0)
 const read=useCallback(async()=>{if(!enabled||!connection?.connected)return;const turn=++revision.current;setBusy(true);setError(null);try{const value=await invoke<History>('get_deepseek_console_history');if(revision.current===turn)setHistory(value)}catch(e){if(revision.current===turn){setHistory(null);setError(String(e))}}finally{if(revision.current===turn)setBusy(false)}},[enabled,connection?.connected])
 useEffect(()=>{let live=true;void invoke<Connection>('get_deepseek_console_connection').then(v=>{if(live)setConnection(v??{connected:false})}).catch(e=>{if(live)setError(String(e))});return()=>{live=false}},[])
 useEffect(()=>{if(enabled&&connection?.connected)void read();else {setHistory(null);setBusy(false)}return()=>{revision.current++}},[enabled,connection?.connected,read])
 const action=async(command:string)=>{revision.current++;setBusy(true);setError(null);try{const result=await invoke<Connection|void>(command);if(result){setConnection(result);setOpened(false);setHistory(null);onChanged()}else setOpened(true)}catch(e){setError(String(e))}finally{setBusy(false)}}
 const days=history?(all?history.days:[...history.days].slice(-7)):[]
 return <section className="group-wrap" aria-label="DeepSeek 控制台账单"><h2>控制台账单</h2><div className="settings-group"><div className="setting-row"><div><strong>{connection?.connected?'已连接控制台':'连接 DeepSeek 控制台'}</strong><small>查看最近 30 天的实际 Token 与扣费。控制台登录独立于 API Key。</small></div><button disabled={busy} onClick={()=>{void action('begin_deepseek_console_login')}}>{connection?.connected?'重新登录':'登录控制台'}</button></div>
 {opened&&<><p className="group-copy">在新窗口中完成 DeepSeek 登录，再返回这里连接。凭据会加密保存。</p><div className="group-actions"><button disabled={busy} onClick={()=>{void action('finish_deepseek_console_login')}}>已登录，连接控制台</button></div></>}
 {connection?.connected&&<div className="group-actions"><button disabled={busy} onClick={()=>{void action('remove_deepseek_console_login')}}>断开控制台</button><button disabled={busy||!enabled} onClick={()=>{void read()}}>刷新账单</button></div>}
 {error&&<p className="group-copy" role="alert">{error}</p>}{busy&&<p className="group-copy" role="status">正在处理控制台…</p>}
 {!enabled&&connection?.connected&&<p className="group-copy">启用 DeepSeek 后读取账单。</p>}
 {history&&<><p className="group-copy">最近检查：{new Date(history.checkedAt).toLocaleString()} · {history.currency??'未取得扣费币种'}</p><div className="group-copy console-billing-table"><table className="spend-table"><thead><tr><th>日期</th><th>输入</th><th>缓存读取</th><th>输出</th><th>Token</th><th>实际扣费</th></tr></thead><tbody>{days.map(d=><tr key={d.date}><th>{d.date}</th><td>{d.input.toLocaleString()}</td><td>{d.cacheRead.toLocaleString()}</td><td>{d.output.toLocaleString()}</td><td title={d.tokens}>{BigInt(d.tokens).toLocaleString()}</td><td>{d.cost===null?'—':`${history.currency??''} ${d.cost.toFixed(4)}`}</td></tr>)}</tbody></table><button onClick={()=>setAll(!all)}>{all?'收起到 7 天':'查看全部 30 天'}</button></div>{history.notes.length>0&&<p className="group-copy" role="alert">{history.notes.join(' ')}</p>}</>}
 <p className="group-copy">API Key 只能读取余额。控制台记录按天汇总，不推断每小时用量；不同币种不会相加。</p></div></section>
}


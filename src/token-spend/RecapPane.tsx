import {useEffect,useMemo,useState} from 'react'
import {invoke} from '@tauri-apps/api/core'
import {buildRecap,recapPeriods,recapPrice,type RecapCard} from './recap'
import {recapImage,renderRecap} from './recap-renderer'
import {exactTokenText} from './summary'
import type {SpendRecord} from './types'
import './recap.css'
const titles:Record<RecapCard,string>={poster:'总览',opener:'开篇',calendar:'月历',yearCalendar:'年历',months:'月份',timetable:'作息',payback:'订阅回报',scorecard:'成绩单'}
export function RecapPane({records,monthlyPrice,hidesProjects,onChange,onClose,partial}:{records:SpendRecord[];monthlyPrice:number|null;hidesProjects:boolean;onChange:(value:{recapMonthlyPrice?:number|null;recapHidesProjects?:boolean})=>void;onClose:()=>void;partial:boolean}){
  const [year,setYear]=useState(false),[key,setKey]=useState(()=>recapPeriods(records,false).defaultKey),[card,setCard]=useState<RecapCard>('poster'),[price,setPrice]=useState(monthlyPrice===null?'':String(monthlyPrice)),[error,setError]=useState<string|null>(null),[busy,setBusy]=useState(false),[outcome,setOutcome]=useState('')
  useEffect(()=>setPrice(monthlyPrice===null?'':String(monthlyPrice)),[monthlyPrice])
  const offered=useMemo(()=>recapPeriods(records,year),[records,year]),recap=useMemo(()=>buildRecap(records,key,monthlyPrice,new Date(),partial),[records,key,monthlyPrice,partial])
  const selected=recap.cards.includes(card)?card:recap.cards[0]
  const image=useMemo(()=>selected?renderRecap(recap,selected,hidesProjects).toDataURL('image/png'):null,[recap,selected,hidesProjects])
  const changeYear=(next:boolean)=>{setYear(next);const keys=recapPeriods(records,next).keys;setKey(next?key.slice(0,4):keys.find(k=>k.startsWith(key.slice(0,4)))??keys[0]);setOutcome('')}
  const commit=()=>{const result=recapPrice(price);if(result==='invalid'){setError('价格须为 0–10000 美元，最多两位小数；留空清除。');setPrice(monthlyPrice===null?'':String(monthlyPrice));return}setError(null);onChange({recapMonthlyPrice:result})}
  const exportCard=async(action:'copy'|'save'|'saveAll')=>{
    if(!selected||busy)return;setBusy(true);setError(null);setOutcome('')
    try{const cards=[];for(const id of action==='saveAll'?recap.cards:[selected]){cards.push(recapImage(recap,id,hidesProjects));await new Promise(r=>setTimeout(r,0))}const result=await invoke<string>('recap_export',{cards,action});setOutcome(result==='saved'?'回顾图片已保存':result==='copied'?'已复制回顾图片':'已取消保存')}
    catch(e){setError(String(e))}finally{setBusy(false)}
  }
  return <section className="recap-pane" aria-label="月度与年度回顾"><div className="recap-toolbar"><h2>工作回顾</h2><button disabled={busy} onClick={onClose}>关闭回顾</button></div>
    <div className="recap-toolbar"><div className="spend-segments" role="group" aria-label="回顾类型"><button aria-pressed={!year} disabled={busy} onClick={()=>changeYear(false)}>月度</button><button aria-pressed={year} disabled={busy} onClick={()=>changeYear(true)}>年度</button></div><select aria-label="回顾日期" value={key} disabled={busy} onChange={e=>{setKey(e.target.value);setOutcome('')}}>{offered.keys.map(k=><option key={k} value={k}>{k}</option>)}</select><label>月订阅价 US$ <input aria-label="回顾月订阅价" inputMode="decimal" value={price} disabled={busy} onChange={e=>setPrice(e.target.value)} onBlur={commit} onKeyDown={e=>{if(e.key==='Enter')commit()}} placeholder="未填写"/></label><label><input type="checkbox" checked={hidesProjects} disabled={busy} onChange={e=>onChange({recapHidesProjects:e.target.checked})}/>隐藏项目名称</label></div>
    {error&&<p role="alert">{error}</p>}{outcome&&<p role="status">{outcome}</p>}
    {!image?<p className="spend-empty">这个时期没有读到 Token 记录，无法生成回顾。</p>:<><p className="spend-footnote">{key} · {exactTokenText(recap.summary.tokens,recap.summary.exactTokens)} Token{partial?' · 计数可能不完整':''}</p><div className="recap-tabs" role="group" aria-label="回顾卡片">{recap.cards.map(id=><button key={id} aria-pressed={selected===id} disabled={busy} onClick={()=>{setCard(id);setOutcome('')}}>{titles[id]}</button>)}</div><div className="recap-preview"><img src={image} width="1080" height="1920" alt={`${key} ${titles[selected]}，${exactTokenText(recap.summary.tokens,recap.summary.exactTokens)} Token`}/></div><div className="recap-toolbar"><button disabled={busy} onClick={()=>{void exportCard('copy')}}>复制图片</button><button disabled={busy} onClick={()=>{void exportCard('save')}}>保存图片</button><button disabled={busy} onClick={()=>{void exportCard('saveAll')}}>保存全部卡片</button>{busy&&<span role="status">正在导出…</span>}</div><p className="spend-footnote">每张图片为 1080 × 1920。仅使用已读取的历史；缺少价格的费用为下限，未来日期和首次记录之前保持未知。</p></>}
  </section>
}

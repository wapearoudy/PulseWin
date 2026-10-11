// Full-size canvas export, independent of the preview's scale.
import { recapFilename, type Recap, type RecapCard } from './recap'
import { dayKey, exactTokenText, formatCost, shortTokens } from './summary'
const paper='#F5F5F1',ink='#1B1B1E',lime='#C8F03C',hairline='#E4E4DD',secondary='#6E6E68',heat=['#EDEDE7','#EEF7C9','#DDF290',lime,'#9DC41A',ink]
export function renderRecap(recap:Recap,card:RecapCard,hidesProjects:boolean):HTMLCanvasElement{
  const canvas=document.createElement('canvas');canvas.width=1080;canvas.height=1920
  const c=canvas.getContext('2d');if(!c)throw new Error('无法创建回顾图片')
  const text=(s:string,x:number,y:number,size=28,bold=false,colour=ink,max=920)=>{c.fillStyle=colour;c.font=`${bold?700:400} ${size}px "Segoe UI", "Microsoft YaHei", sans-serif`;c.fillText(s,x,y,max)}
  const box=(x:number,y:number,w:number,h:number,fill='#FFFFFF')=>{c.fillStyle=fill;c.strokeStyle=fill==='#FFFFFF'?hairline:fill;c.lineWidth=1;c.beginPath();c.roundRect(x,y,w,h,28);c.fill();c.stroke()}
  const stat=(label:string,value:string,x:number,y:number,w=440,fill='#FFFFFF')=>{box(x,y,w,180,fill);text(label,x+26,y+48,26,false,fill===ink?paper:secondary,w-52);text(value,x+26,y+125,62,true,fill===ink?paper:ink,w-52)}
  const ranks=(groups:{name:string;tokens:number}[],y:number,projects=false)=>{const top=groups.slice(0,5),max=Math.max(1,...top.map(g=>g.tokens));top.forEach((g,i)=>{text(projects&&hidesProjects?`Project ${i+1}`:g.name,106,y+i*100,28,false,ink,650);text(shortTokens(g.tokens),760,y+i*100,28,true,ink,215);c.fillStyle=i===0?lime:i===1?ink:'#9A9A93';c.fillRect(106,y+20+i*100,850*g.tokens/max,16)})}
  const calendar=(month:number,x:number,y:number,w:number)=>{
    const start=new Date(recap.start.getFullYear(),month-1,1),last=new Date(start.getFullYear(),month,0).getDate(),offset=(start.getDay()+6)%7,size=w/7,peak=Math.max(1,...recap.summary.days.map(d=>d.tokens))
    text(`${month} 月`,x,y,Math.max(22,size*.33),true);for(let d=1;d<=last;d++){const date=new Date(start.getFullYear(),month-1,d),index=offset+d-1,col=index%7,row=Math.floor(index/7),item=recap.summary.days.find(r=>r.day===dayKey(date)),future=date>=recap.end,unrecorded=recap.recordsBegin!==null&&date<recap.recordsBegin,tokens=item?.tokens??0,px=x+col*size,py=y+30+row*size;c.beginPath();c.roundRect(px+3,py+3,size-6,size-6,Math.min(12,size*.15));c.fillStyle=future||unrecorded?paper:heat[tokens<=0?0:tokens===peak?5:Math.min(4,Math.ceil(tokens/peak*4))];c.fill();if(future||unrecorded){c.setLineDash([3,3]);c.strokeStyle=hairline;c.stroke();c.setLineDash([])}text(String(d),px+size*.2,py+size*.65,Math.max(13,size*.25),false,tokens===peak?paper:future||unrecorded?'#A3A39C':ink,size-8)}
  }
  c.fillStyle=paper;c.fillRect(0,0,1080,1920);text('PULSE RECAP',80,104,20,true)
  const stories=recap.cards.filter(k=>k!=='poster'),number=card==='poster'?0:stories.indexOf(card)+1;if(card!=='poster')text(`${String(number).padStart(2,'0')} / ${String(stories.length).padStart(2,'0')}`,880,104,20,false,secondary)
  text(recap.key.replace('-', ' · '),80,204,36,true);text(recap.isInProgress?'截至今天':'完整回顾',80,251,25,false,secondary)
  const s=recap.summary
  if(card==='poster'){
    text(recap.isYear?'这一年的工作':'这个月的工作',80,370,82,true);text(shortTokens(s.tokens),80,535,132,true);text('TOKEN',85,586,30,false,secondary)
    stat('活跃日',String(s.activeDays),80,652,440,lime);stat('会话',String(s.sessions.length),560,652)
    stat('API 估算费用',formatCost(s.cost),80,862);stat(recap.payback!==null?'订阅回报':'最长连续工作',recap.payback!==null?`${recap.payback.toFixed(1)} ×`:`${recap.longestStreak} 天`,560,862,440,ink)
    text('工作伙伴',80,1130,28,true);ranks(s.agents,1190);text('模型与项目',80,1720,28,true);text(s.models.slice(0,2).map(g=>g.name).join(' / '),80,1770,25,false,secondary)
  }else if(card==='opener'){
    text(recap.isYear?String(recap.start.getFullYear()):String(recap.start.getMonth()+1).padStart(2,'0'),80,690,340,true);text(recap.isYear?'年度回顾':'月度回顾',80,805,96,true);text('一起完成的工作',80,980,40,true);ranks(s.agents,1080)
  }else if(card==='calendar'||card==='yearCalendar'){
    text('每一个工作日',80,355,74,true);if(card==='calendar'){calendar(recap.start.getMonth()+1,80,510,920);text(`${s.activeDays} 个活跃日 · ${recap.observedDays} 个有记录的日期`,80,1520,32,true);if(recap.busiest)text(`最忙的一天：${recap.busiest.day} · ${shortTokens(recap.busiest.tokens)} Token`,80,1590,28,false,secondary)}else{for(let m=1;m<=12;m++)calendar(m,80+(m-1)%3*315,480+Math.floor((m-1)/3)*315,290)}
  }else if(card==='months'){
    text('一年的节奏',80,355,80,true);const peak=Math.max(1,...recap.months.map(m=>m.tokens));recap.months.forEach((m,i)=>{const y=500+i*94;text(`${m.month} 月`,80,y,28);c.fillStyle=m.future?paper:m.tokens===peak?lime:ink;c.strokeStyle=hairline;c.fillRect(178,y-26,m.future?1:Math.max(1,m.tokens/peak*600),32);if(m.future){c.setLineDash([4,4]);c.strokeRect(178,y-26,600,32);c.setLineDash([])}text(m.future?'尚未开始':shortTokens(m.tokens),805,y,28,true,secondary,195)})
  }else if(card==='timetable'){
    text(recap.persona??'工作的时刻',80,355,88,true);text(`高峰 ${String(recap.peakHour).padStart(2,'0')}:00`,80,470,45,true);const peak=Math.max(...recap.hours,1);recap.hours.forEach((t,h)=>{c.fillStyle=h===recap.peakHour?lime:ink;c.fillRect(80+h*38,1070-t/peak*400,28,Math.max(1,t/peak*400))});text('00:00',80,1120,25,false,secondary);text('12:00',510,1120,25,false,secondary);text('23:00',885,1120,25,false,secondary);if(recap.lateNights!==null)stat('跨午夜工作',String(recap.lateNights),80,1230);if(recap.latestMinute!==null)stat('最晚结束',`${String(Math.floor(recap.latestMinute/60)%24).padStart(2,'0')}:${String(recap.latestMinute%60).padStart(2,'0')}`,560,1230);if(recap.untimedTokens>0)text(`${shortTokens(recap.untimedTokens)} Token 没有精确时刻，未计入节奏`,80,1530,26,false,secondary)
  }else if(card==='payback'){
    text('这份订阅的回报',80,355,78,true);text(`${recap.payback?.toFixed(1)} ×`,80,735,200,true);box(80,835,920,220,lime);text('同样的工作，按 API 价格估算',106,895,30);text(formatCost(s.cost),106,990,76,true);stat('同期订阅费用',formatCost(recap.paid),80,1140,920);text(recap.isInProgress?'订阅费用按已过日期折算。':'按有记录的日期计算订阅费用。',80,1420,30,false,secondary);if(s.unpricedTokens)text('部分工作没有公开价格，回报是下限。',80,1500,30,false,secondary)
  }else{
    text('工作留下的数字',80,355,80,true);stat('Token',shortTokens(s.tokens),80,450,920,lime);stat('活跃日',String(s.activeDays),80,670);stat('连续工作',`${recap.longestStreak} 天`,560,670);if(recap.cacheHitRate!==null)stat('缓存命中率',`${(recap.cacheHitRate*100).toFixed(1)}%`,80,890);stat('API 估算费用',formatCost(s.cost),560,890);if(recap.cacheSavings!==null&&recap.cacheSavings>=.5)text(`缓存节省至少 ${formatCost(recap.cacheSavings)}`,80,1140,28,true);text('项目',80,1190,32,true);ranks(s.projects,1260,true);if(recap.cacheUnmeasuredTokens>0)text(`${shortTokens(recap.cacheUnmeasuredTokens)} Token 的来源未测量缓存，未计入命中率`,80,1710,24,false,secondary);if(recap.change!==null)text(`每日 Token 比上一期 ${recap.change>=0?'↑':'↓'} ${Math.abs(recap.change*100).toFixed(0)}%`,80,1750,28,true)
  }
  const notes=[...(recap.isPartial?['计数可能不完整；请在 Token 消耗页查看读取说明。']:[]),'本机已读取的历史记录 · models.dev 公开价格 · USD',...(s.unpricedTokens?['部分费用未定价，金额为已定价部分的下限。']:[]),...(recap.recordsBegin?[`记录从 ${dayKey(recap.recordsBegin)} 开始；此前未知。`]:[])];notes.slice(0,3).forEach((n,i)=>text(n,80,1820+i*27,20,false,secondary))
  // Exact integer figures remain available in the accessible preview, while the image uses compact units.
  canvas.setAttribute('aria-label',`${recap.key} ${card} ${exactTokenText(s.tokens,s.exactTokens)} Token`)
  return canvas
}
export const recapImage=(r:Recap,card:RecapCard,privacy:boolean)=>({name:recapFilename(r,card),png:renderRecap(r,card,privacy).toDataURL('image/png').split(',')[1]})

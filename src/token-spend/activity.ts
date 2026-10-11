// Pulse 1.9 TokenActivity: calendar dates, missing history and quartile colours.
import type { SpendRecord } from './types'
import { dayKey, exactRecordTokens, modelKey, totalTokens } from './summary'
export type ActivityView='daily'|'weekly'|'cumulative'
export interface ActivityDay {date:string;tokens:number;exactTokens:string|null}
export interface ActivityWeek {start:string;days:(ActivityDay|null)[];unrecorded:boolean[];tokens:number;exactTokens:string|null}
export interface TokenActivity {weeks:ActivityWeek[];total:number;exactTotal:string|null;activeDays:number;cuts:number[];marks:{label:string;position:number}[]}
const addDays=(d:Date,n:number)=>new Date(d.getFullYear(),d.getMonth(),d.getDate()+n)
const monday=(d:Date)=>addDays(d,-((d.getDay()+6)%7))
export function activityWindow(now=new Date()){
  const today=new Date(now.getFullYear(),now.getMonth(),now.getDate())
  const previousYear=now.getFullYear()-1,last=new Date(previousYear,now.getMonth()+1,0).getDate()
  const candidate=addDays(new Date(previousYear,now.getMonth(),Math.min(now.getDate(),last)),1)
  const longest=addDays(monday(today),-52*7),start=candidate>longest?candidate:longest
  return {start,gridStart:monday(start),today}
}
export function tokenActivity(records:SpendRecord[],now=new Date(),agent?:string|null,model?:string|null):TokenActivity{
  const result:TokenActivity={weeks:[],total:0,exactTotal:'0',activeDays:0,cuts:[],marks:[]}
  const {start,gridStart,today}=activityWindow(now),startKey=dayKey(start),todayKey=dayKey(today)
  let first:string|null=null
  const sums=new Map<string,{tokens:number;exact:bigint|null}>()
  for(const r of records){
    if(agent&&r.agent!==agent || model&&modelKey(r)!==model || totalTokens(r)<=0 || r.day>todayKey)continue
    if(first===null||r.day<first)first=r.day
    if(r.day<startKey)continue
    const old=sums.get(r.day)??{tokens:0,exact:BigInt(0)},count=exactRecordTokens(r)
    old.tokens+=totalTokens(r);old.exact=old.exact===null||count===null?null:old.exact+count;sums.set(r.day,old)
  }
  if(first===null)return result
  const recordedFrom=first>startKey?first:startKey
  let exact:bigint|null=BigInt(0),month='',lastMark=-Infinity
  for(let cursor=gridStart;cursor<=today;cursor=addDays(cursor,7)){
    const week:ActivityWeek={start:dayKey(cursor),days:[],unrecorded:[],tokens:0,exactTokens:'0'};let weekExact:bigint|null=BigInt(0)
    for(let row=0;row<7;row++){
      const date=addDays(cursor,row),key=dayKey(date),inside=key>=startKey&&key<=todayKey
      week.unrecorded.push(inside&&key<recordedFrom)
      if(inside&&key>=recordedFrom){const s=sums.get(key)??{tokens:0,exact:BigInt(0)};week.days.push({date:key,tokens:s.tokens,exactTokens:s.exact?.toString()??null});week.tokens+=s.tokens;weekExact=weekExact===null||s.exact===null?null:weekExact+s.exact;result.total+=s.tokens;exact=exact===null||s.exact===null?null:exact+s.exact;if(s.tokens>0)result.activeDays++}else{week.days.push(null)}
      if(inside&&key.slice(0,7)!==month){month=key.slice(0,7);const position=result.weeks.length+row/7;if(position-lastMark<3)result.marks.pop();result.marks.push({label:`${date.getMonth()+1}月`,position});lastMark=position}
    }
    week.exactTokens=weekExact?.toString()??null;result.weeks.push(week)
  }
  result.exactTotal=exact?.toString()??null
  const busy=result.weeks.flatMap(w=>w.days).filter((d):d is ActivityDay=>!!d&&d.tokens>0).map(d=>d.tokens).sort((a,b)=>a-b)
  if(busy.length)result.cuts=[1,2,3].map(q=>busy[Math.min(busy.length-1,Math.max(Math.ceil(busy.length*q/4)-1,0))])
  return result
}
export const activityStep=(tokens:number,cuts:number[])=>tokens>0?1+cuts.filter(c=>tokens>c).length:0

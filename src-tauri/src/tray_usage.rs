//! The tray reads the published snapshot. Opening it never asks a provider.
use crate::{model::{ProviderUsage, Snapshot, UsageWindow}, preferences::Preferences};

pub fn headline<'a>(p: &'a ProviderUsage, prefs: &Preferences) -> Option<&'a UsageWindow> {
    prefs.pinned_windows.get(&p.id).and_then(|pin| p.windows.iter().find(|w| w.id.as_ref()==Some(pin)||&w.label==pin))
        .or_else(|| p.windows.iter().max_by(|a,b| a.percent_used.filter(|v|v.is_finite()).unwrap_or(-1.0)
            .total_cmp(&b.percent_used.filter(|v|v.is_finite()).unwrap_or(-1.0))))
}
pub fn selected<'a>(snapshot: &'a Snapshot, prefs: &Preferences) -> Option<&'a ProviderUsage> {
    let shown = |p: &&ProviderUsage| prefs.enabled_providers.contains(&p.id);
    if let Some(id)=&prefs.tray_account {
        if let Some(p)=snapshot.providers.iter().filter(shown).find(|p|&p.id==id) {return Some(p)}
    }
    // Strict > preserves the first rail account when two figures tie.
    let mut best=None;let mut used=-1.0;
    for p in snapshot.providers.iter().filter(shown) {
        if let Some(value)=headline(p,prefs).and_then(|w|w.percent_used).filter(|v|v.is_finite()) {
            if value>used {best=Some(p);used=value;}
        }
    }
    best
}
pub fn figure(used:f64,remaining:bool)->u32 {
    let n=if remaining{100.0-used}else{used};
    if n<=0.0 {0} else if n>=100.0 {100} else {n.round().clamp(1.0,99.0) as u32}
}
pub fn split(p:&ProviderUsage)->Vec<&UsageWindow> {
    let mut windows=p.windows.iter().filter(|w|w.scope.is_none() && w.percent_used.is_some_and(|v|v.is_finite()) &&
        w.window_seconds.is_some_and(|s|s.is_finite() && [18000.0,86400.0,604800.0,2592000.0,2678400.0].contains(&s))).collect::<Vec<_>>();
    windows.sort_by(|a,b|a.window_seconds.unwrap().total_cmp(&b.window_seconds.unwrap()));
    windows.dedup_by(|a,b|a.window_seconds.map(|s|s.min(2592000.0))==b.window_seconds.map(|s|s.min(2592000.0)));
    if windows.len()<2 {vec![]} else {windows.truncate(2);windows}
}
pub fn tooltip(snapshot:&Snapshot,prefs:&Preferences)->String {
    let Some(p)=selected(snapshot,prefs) else{return "PulseWin · 点击查看用量概览".into()};
    let mut lines=vec![format!("PulseWin · {}{}",p.name,if p.stale{"（旧读数）"}else{""})];
    for w in &p.windows {
        let value=w.percent_used.filter(|v|v.is_finite()).map(|v|format!("{}% {}",figure(v,prefs.shows_remaining),if prefs.shows_remaining{"剩余"}else{"已用"})).unwrap_or_else(||"暂无数字".into());
        lines.push(format!("{} · {}{}",w.label,value,if w.is_exhausted{" · 已限额"}else{""}));
    }
    if let Some(c)=&p.credit_remaining {if c.amount.is_finite(){lines.push(format!("余额 {} {:.2}",c.currency,c.amount));}}
    if p.windows.is_empty() && p.credit_remaining.is_none(){lines.push("暂无用量数字".into());}
    // Windows limits notification-area tooltips to 127 UTF-16 code units.
    let mut out=String::new();let mut units=0;
    for ch in lines.join("\n").chars(){if units+ch.len_utf16()>126{out.push('…');break}out.push(ch);units+=ch.len_utf16();}
    out
}

// Vector ring + pixel font rendered at 64px and downsampled to 32px. The dark
// outline keeps figures legible on both Windows taskbar colour schemes.
pub fn icon(p:&ProviderUsage,prefs:&Preferences)->Vec<u8> {
    let mut image=vec![0u8;64*64*4];
    let windows=if prefs.tray_style=="split"{split(p)}else{vec![]};
    let tint=|w:&UsageWindow| if w.is_exhausted||w.percent_used.unwrap_or(0.0)>=prefs.warning_threshold*100.0 {[255,69,58,255]}else{[64,163,255,255]};
    if windows.len()==2 {
        for (i,w) in windows.iter().enumerate(){digits(&mut image,&figure(w.percent_used.unwrap(),prefs.shows_remaining).to_string(),(i*30+4) as i32,3,tint(w));}
    }else if let Some(w)=headline(p,prefs) {
        if let Some(used)=w.percent_used.filter(|v|v.is_finite()) {
            if prefs.tray_style=="ring" {
                let fraction=if w.is_exhausted{1.0}else if prefs.shows_remaining{1.0-used/100.0}else{used/100.0}.clamp(0.0,1.0);
                for y in 0..64 {for x in 0..64 {
                    let dx=x as f64-31.5;let dy=y as f64-31.5;let distance=dx.hypot(dy);
                    if (22.0..29.0).contains(&distance) {
                        let phase=(dx.atan2(-dy)+std::f64::consts::TAU)%std::f64::consts::TAU;
                        paint(&mut image,x,y,if phase<=fraction*std::f64::consts::TAU && fraction>0.0 {tint(w)}else{[128,128,128,130]});
                    }
                }}
                digits(&mut image,&figure(used,prefs.shows_remaining).to_string(),24,2,tint(w));
            }else{digits(&mut image,&figure(used,prefs.shows_remaining).to_string(),18,4,tint(w));}
        }else{digits(&mut image,"-",18,4,[160,160,160,255]);}
    }else{digits(&mut image,"-",18,4,[160,160,160,255]);}
    let mut small=vec![0u8;32*32*4];
    for y in 0..32 {for x in 0..32 {
        let samples=[(2*x,2*y),(2*x+1,2*y),(2*x,2*y+1),(2*x+1,2*y+1)];
        let alpha=samples.iter().map(|(sx,sy)|image[(sy*64+sx)*4+3] as u32).sum::<u32>();
        for c in 0..3 {small[(y*32+x)*4+c]=if alpha==0{0}else{(samples.iter().map(|(sx,sy)|image[(sy*64+sx)*4+c] as u32*image[(sy*64+sx)*4+3] as u32).sum::<u32>()/alpha) as u8};}
        small[(y*32+x)*4+3]=(alpha/4) as u8;
    }}
    small
}
fn paint(image:&mut [u8],x:i32,y:i32,colour:[u8;4]){if (0..64).contains(&x)&&(0..64).contains(&y){image[((y*64+x)*4) as usize..((y*64+x)*4+4) as usize].copy_from_slice(&colour);}}
fn digits(image:&mut [u8],text:&str,y:i32,scale:i32,colour:[u8;4]){
    const FONT:[[u8;7];11]=[
        [14,17,19,21,25,17,14],[4,12,4,4,4,4,14],[14,17,1,2,4,8,31],[30,1,1,14,1,1,30],[2,6,10,18,31,2,2],
        [31,16,16,30,1,1,30],[14,16,16,30,17,17,14],[31,1,2,4,8,8,8],[14,17,17,14,17,17,14],[14,17,17,15,1,1,14],[0,0,0,31,0,0,0]];
    let scale=if text.len()==3 {scale.min(3)} else {scale};
    let left=(64-(text.len() as i32*6-1)*scale)/2;
    for outline in [true,false] {for (i,ch) in text.bytes().enumerate(){let glyph=FONT[if ch==b'-'{10}else{(ch-b'0') as usize}];
        for (row,bits) in glyph.iter().enumerate(){for col in 0..5 {if bits&(1<<(4-col))!=0 {
            for dy in 0..scale {for dx in 0..scale {
                let x=left+(i as i32*6+col)*scale+dx;let y=y+row as i32*scale+dy;
                if outline {for oy in -1..=1{for ox in -1..=1{paint(image,x+ox,y+oy,[20,25,35,255]);}}} else {paint(image,x,y,colour);}
            }}
        }}}
    }}
}

pub fn popup_position(anchor:(i32,i32),size:(i32,i32),work:crate::desktop::Rect)->(i32,i32){
    let (w,h)=(size.0.min(work.right-work.left).max(1),size.1.min(work.bottom-work.top).max(1));
    let y=if anchor.1-h-8>=work.top{anchor.1-h-8}else{anchor.1+8};
    ((anchor.0-w/2).clamp(work.left,work.right-w),y.clamp(work.top,work.bottom-h))
}

#[cfg(test)] mod tests {
    use super::*;
    fn prefs()->Preferences{Preferences{enabled_providers:vec!["a".into(),"b".into()],..Default::default()}}
    fn snapshot()->Snapshot{Snapshot{providers:vec![ProviderUsage::ok("a","A",vec![UsageWindow::new("5h",Some(80.0))]),ProviderUsage::ok("b","B",vec![UsageWindow::new("7d",Some(80.0))])],fetched_at:String::new()}}
    #[test]fn tie_pin_and_disabled_selection_follow_rail(){let mut s=snapshot();let mut p=prefs();assert_eq!(selected(&s,&p).unwrap().id,"a");p.tray_account=Some("b".into());assert_eq!(selected(&s,&p).unwrap().id,"b");p.enabled_providers.pop();assert_eq!(selected(&s,&p).unwrap().id,"a");s.providers[0].windows.push(UsageWindow::new("pinned",Some(2.0)));p.pinned_windows.insert("a".into(),"pinned".into());assert_eq!(headline(&s.providers[0],&p).unwrap().percent_used,Some(2.0));}
    #[test]fn balance_or_unknown_is_not_an_automatic_zero(){let mut s=snapshot();for p in &mut s.providers{p.windows.clear();}s.providers[0].credit_remaining=Some(crate::model::CreditRemaining{amount:5.0,currency:"USD".into()});assert!(selected(&s,&prefs()).is_none());let mut p=prefs();p.tray_account=Some("a".into());assert!(tooltip(&s,&p).contains("USD 5.00"));assert!(!tooltip(&s,&p).contains("0%"));}
    #[test]fn percentage_never_rounds_to_a_false_endpoint(){assert_eq!(figure(0.01,false),1);assert_eq!(figure(99.99,false),99);assert_eq!(figure(100.0,true),0);}
    #[test]fn split_needs_two_distinct_evidenced_account_wide_windows(){let mut p=snapshot().providers.remove(0);p.windows=vec![UsageWindow::new("5h",Some(10.0)).with_duration(Some(18000.0)),UsageWindow::new("model",Some(90.0)).with_duration(Some(604800.0)).with_scope(Some("model".into()))];assert!(split(&p).is_empty());p.windows.push(UsageWindow::new("week",Some(20.0)).with_duration(Some(604800.0)));assert_eq!(split(&p).len(),2);}
    #[test]fn all_styles_produce_transparent_nonempty_icons(){let s=snapshot();let mut p=prefs();for style in ["figure","ring","split"]{p.tray_style=style.into();let i=icon(&s.providers[0],&p);assert_eq!(i.len(),4096);assert!(i.chunks(4).any(|c|c[3]>0));assert_eq!(i[3],0);}}
    #[test]fn popup_stays_inside_negative_work_area_and_top_taskbar(){let r=crate::desktop::Rect{left:-1920,top:40,right:0,bottom:1080};assert_eq!(popup_position((-5,1079),(480,840),r),(-480,231));assert_eq!(popup_position((-1919,41),(480,840),r),(-1920,49));assert_eq!(popup_position((0,0),(3000,2000),r),(-1920,40));}
    #[test]fn tooltip_fits_windows_utf16_limit(){let mut s=snapshot();s.providers[0].name="😀".repeat(150);assert!(tooltip(&s,&prefs()).encode_utf16().count()<=127);}
}

//! Read-only status-line output. No HTTP client, credential access or GUI startup.
use crate::{model::{UsageWindow,WindowKind}, preferences::Preferences,usage_cache::{UsageCache,timestamp}};
use serde_json::{json,Value};

fn kind(window:&UsageWindow)->String {
    match window.kind {
        WindowKind::Balance=>"balance".into(),WindowKind::TopUp=>"topUp".into(),
        WindowKind::Credits=>"credits".into(),WindowKind::SharedCredits=>"sharedCredits".into(),
        _=>match window.window_seconds.map(|s|s as u64) {
            Some(18000)=>"fiveHour".into(),Some(86400)=>"daily".into(),Some(604800)=>"weekly".into(),
            Some(seconds)=>format!("other:{seconds}"),None=>"other".into(),
        }
    }
}
fn figure(percent:Option<f64>)->Option<u32>{percent.filter(|v|v.is_finite()).map(|v|
    if v<=0.0 {0} else if v>=100.0 {100} else {(v.round() as u32).clamp(1,99)})}
fn window_value(window:&UsageWindow)->Value {
    let used=figure(window.percent_used);
    json!({"id":window.id,"scope":window.scope,"kind":kind(window),"usedFraction":window.percent_used.map(|v|v/100.0),
        "usedPercent":used,"percentText":used.map(|v|format!("{v}%")),
        "isExhausted":window.is_exhausted,"resetsAt":window.resets_at,
        "reportsLength":window.window_seconds.is_some(),"windowSeconds":window.window_seconds})
}
pub fn encode(prefs:&Preferences,cache:&UsageCache,now:i64)->Value {
    let registry=crate::providers::registry();
    let mut ids=prefs.provider_order.iter().filter(|id|prefs.enabled_providers.contains(id)).cloned().collect::<Vec<_>>();
    for id in &prefs.enabled_providers{if !ids.contains(id){ids.push(id.clone());}}
    let accounts=ids.into_iter().filter_map(|id| {
        let base=prefs.accounts.iter().find(|a|a.id==id).map(|a|a.provider.as_str()).unwrap_or(&id);
        let provider=registry.iter().find(|p|p.id()==base)?;
        let label=prefs.account_labels.get(&id).map(String::as_str).unwrap_or(provider.name());
        let reading=cache.readings.get(&id);
        let windows=reading.map(|p|p.windows.as_slice()).unwrap_or(&[]);
        let headline=prefs.pinned_windows.get(&id).and_then(|label|windows.iter().find(|w|w.id.as_ref()==Some(label)||&w.label==label))
            .or_else(||windows.iter().filter(|w|w.percent_used.is_some()).max_by(|a,b|a.percent_used.unwrap().total_cmp(&b.percent_used.unwrap())));
        Some(json!({"id":id,"provider":base,"name":provider.name(),"label":label,
            "plan":reading.and_then(|p|p.plan.as_deref()),
            "creditBalance":reading.and_then(|p|p.credit_remaining.as_ref()).map(|b|format!("{} {:.2}",b.currency,b.amount)),
            "observedAt":reading.map(|p|p.fetched_at.as_str()),
            "ageSeconds":reading.and_then(|p|timestamp(&p.fetched_at)).map(|t|(now-t).max(0)),
            "source":reading.and_then(|p|p.source.as_deref()),
            "headline":headline.map(window_value),"windows":windows.iter().map(window_value).collect::<Vec<_>>()}))
    }).collect::<Vec<_>>();
    json!({"generatedAt":chrono::DateTime::from_timestamp(now,0).map(|v|v.to_rfc3339()),"accounts":accounts})
}
pub fn print(statusline:bool)->Result<(),String>{
    let value=encode(&crate::preferences::load(),&UsageCache::load(),chrono::Utc::now().timestamp());
    if statusline {
        let text=value["accounts"].as_array().unwrap().iter().filter_map(|account|
            account["headline"]["percentText"].as_str().map(|pct|format!("{} {pct}",account["label"].as_str().unwrap_or("Pulse"))))
            .collect::<Vec<_>>().join(" · ");println!("{text}");
    } else {println!("{}",serde_json::to_string_pretty(&value).map_err(|e|e.to_string())?);}
    Ok(())
}

#[cfg(test)] mod tests {
    use super::*;
    use crate::model::ProviderUsage;
    #[test] fn empty_configuration_never_invents_an_account() {
        assert!(encode(&Preferences::default(),&UsageCache::default(),1000)["accounts"].as_array().unwrap().is_empty());
    }
    #[test] fn report_keeps_same_provider_accounts_and_labels_separate() {
        let id="codex--account-ab";
        let mut prefs=Preferences{enabled_providers:vec!["codex".into(),id.into()],provider_order:vec![id.into(),"codex".into()],..Default::default()};
        prefs.accounts.push(crate::accounts::Account{id:id.into(),provider:"codex".into()});prefs.account_labels.insert(id.into(),"工作".into());
        let mut cache=UsageCache::default();
        for (key,percent) in [("codex",12.0),(id,68.0)] {cache.readings.insert(key.into(),ProviderUsage::ok(key,"Codex",vec![UsageWindow::new("5h",Some(percent))]));}
        let report=encode(&prefs,&cache,1000);
        assert_eq!(report["accounts"][0]["id"],id);assert_eq!(report["accounts"][0]["provider"],"codex");
        assert_eq!(report["accounts"][0]["label"],"工作");assert_eq!(report["accounts"][0]["headline"]["usedPercent"],68);
        assert_eq!(report["accounts"][1]["headline"]["usedPercent"],12);
    }
    #[test] fn report_uses_order_pin_real_observation_and_independent_display_rounding() {
        let prefs=Preferences{enabled_providers:vec!["codex".into()],provider_order:vec!["codex".into()],..Default::default()};
        let mut cache=UsageCache::default();let mut p=ProviderUsage::ok("codex","Codex",vec![UsageWindow::new("week",Some(99.6))]);
        p.fetched_at=chrono::DateTime::from_timestamp(1000,0).unwrap().to_rfc3339();cache.readings.insert("codex".into(),p);
        let report=encode(&prefs,&cache,1060);assert_eq!(report["accounts"][0]["ageSeconds"],60);
        assert_eq!(report["accounts"][0]["headline"]["usedPercent"],99);
        assert_eq!(report["accounts"][0]["headline"]["isExhausted"],false);
        assert!(report["accounts"][0]["source"].is_null());
    }
}

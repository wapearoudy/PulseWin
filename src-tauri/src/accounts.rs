//! Explicit account identities and credential snapshots.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
pub const SUPPORTED: &[&str] = &["claude-code", "codex", "grok", "grok-bot"];
pub const SEPARATOR: &str = "--account-";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all="camelCase")]
pub struct Account { pub id: String, pub provider: String }
pub fn valid(account: &Account) -> bool {
    SUPPORTED.contains(&account.provider.as_str()) &&
        account.id.strip_prefix(&format!("{}{SEPARATOR}", account.provider))
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_hexdigit())) &&
        crate::credential_store::valid_provider_id(&account.id)
}
pub fn new_id(provider: &str) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("{provider}{SEPARATOR}{nanos:x}{:x}", SEQUENCE.fetch_add(1,Ordering::Relaxed))
}
pub fn label(value: &str) -> Result<String, String> {
    let value=value.trim();
    if value.is_empty() || value.chars().count()>64 || value.chars().any(char::is_control) {return Err("账号名称需为 1–64 个字符。".into());}
    Ok(value.to_owned())
}
pub fn token(document: &Value) -> Option<String> {
    crate::credentials::dig_first_str(document, &["apiKey", "token", "cookie", "claudeAiOauth.accessToken", "claudeAiOauth.access_token", "tokens.access_token", "accessToken", "access_token"])
}
pub fn document(provider: &str, input: &str) -> Result<Value, String> {
    if !SUPPORTED.contains(&provider) {return Err("此服务暂不支持附加账号。".into());}
    if input.len()>1024*1024 {return Err("凭据内容过大。".into());}
    let input=input.trim();
    let mut value=if input.starts_with('{') {serde_json::from_str::<Value>(input).map_err(|_|"登录 JSON 格式无效。".to_string())?} else {json!({"apiKey":input})};
    if !value.is_object(){return Err("登录信息必须是 JSON 对象。".into());}
    let key=if provider=="grok" {crate::providers::grok::account_token(&value)?} else {token(&value).ok_or("没有找到 OAuth Token 或会话 Cookie。")?};
    if key.trim().is_empty() || key.chars().any(char::is_control) {return Err("凭据为空或包含无效换行。".into());}
    let account_id=crate::credentials::dig_first_str(&value,&["accountId","tokens.account_id","account_id"]);
    value["apiKey"]=json!(key.trim());if let Some(id)=account_id {value["accountId"]=json!(id);}
    Ok(value)
}
pub fn local_document(provider: &str) -> Result<Value, String> {
    let paths=match provider {
        "claude-code"=>crate::credentials::claude_credential_paths(),"codex"=>crate::credentials::codex_credential_paths(),
        "grok"=>crate::credentials::home_relative(&[".grok","auth.json"]),_=>return Err("此账号请粘贴独立的会话 Cookie。".into()),
    };
    for path in paths {if let Some(value)=crate::credentials::read_json(&path) {if let Ok(d)=document(provider,&value.to_string()){return Ok(d);}}}
    Err("未找到可导入的 CLI 登录；请先登录该账号，或粘贴登录 JSON / OAuth Token。".into())
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn codex_import_keeps_its_account_and_rejects_api_key_only_login() {
        let d=document("codex",r#"{"tokens":{"access_token":"test-token-b","account_id":"workspace-b"}}"#).unwrap();
        assert_eq!(token(&d).as_deref(),Some("test-token-b"));assert_eq!(d["accountId"],"workspace-b");
        assert!(document("codex",r#"{"OPENAI_API_KEY":"test-key"}"#).is_err());
    }
    #[test] fn identity_cannot_escape_storage_or_impersonate_another_provider() {
        let id=new_id("codex");assert!(valid(&Account{id:id.clone(),provider:"codex".into()}));
        assert!(!valid(&Account{id,provider:"grok".into()}));
        assert!(!valid(&Account{id:"codex--account-../other".into(),provider:"codex".into()}));
        assert!(!valid(&Account{id:"deepseek--account-ab".into(),provider:"deepseek".into()}));
    }
    #[test] fn bad_input_does_not_echo_secrets() {
        assert!(!document("claude-code","{test-secret").unwrap_err().contains("test-secret"));
        assert!(document("claude-code","test\ntoken").is_err());assert!(label("\n").is_err());
    }
    #[test] fn importing_grok_preserves_expiry_instead_of_promoting_an_old_token() {
        let expiry=(chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339();
        let input=json!({"issuer":{"key":"synthetic-grok","expires_at":expiry}});
        let mut d=document("grok",&input.to_string()).unwrap();
        assert_eq!(token(&d).as_deref(),Some("synthetic-grok"));
        d["issuer"]["expires_at"]=json!("2000-01-01T00:00:00Z");
        assert!(crate::providers::grok::account_token(&d).is_err());
    }
    #[tokio::test] async fn missing_added_login_never_falls_back_to_the_primary_cli() {
        let id=new_id("codex");let mut p=crate::preferences::Preferences::default();
        p.accounts.push(Account{id:id.clone(),provider:"codex".into()});
        let ctx=std::sync::Arc::new(crate::providers::Ctx::new().unwrap());
        assert!(crate::providers::collect_accounts(ctx.clone(),&[],&p).await.is_empty());
        let result=crate::providers::collect_accounts(ctx,&[id.clone()],&p).await;
        assert_eq!(result.len(),1);assert_eq!(result[0].id,id);assert!(result[0].configured);
        assert!(result[0].error.as_deref().unwrap().contains("独立登录"));assert!(result[0].windows.is_empty());
    }
}

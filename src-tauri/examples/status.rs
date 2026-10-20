//! Public official-status probe; no credentials or account usage is accessed.
//!
//! ```text
//! cargo run --example status              # every page in the catalogue
//! cargo run --example status -- codex     # one provider's page
//! ```
use pulsewin_lib::{providers::Ctx, service_status::{self, catalogue}};
#[tokio::main]
async fn main(){
    let wanted=std::env::args().nth(1);
    let ctx=Ctx::new().expect("HTTP client");
    for page in catalogue(){
        if wanted.as_deref().is_some_and(|w| w!=page.provider){continue}
        match service_status::read(&ctx.client,page.provider,true).await{
            Ok(r)=>println!("{}",serde_json::to_string(&r).unwrap()),
            Err(e)=>println!("{}: {e}",page.provider),
        }
    }
}

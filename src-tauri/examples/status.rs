//! Public official-status probe; no credentials or account usage is accessed.
use pulsewin_lib::{providers::Ctx,service_status::{self,Page}};
#[tokio::main]
async fn main(){let ctx=Ctx::new().expect("HTTP client");for page in [Page::OpenAI,Page::Claude,Page::DeepSeek]{match service_status::read(&ctx.client,page,true).await{Ok(r)=>println!("{}",serde_json::to_string(&r).unwrap()),Err(e)=>println!("{page:?}: {e}")}}}

//! Manual end-to-end probe for a single provider.
//!
//! ```text
//! cargo run --example probe -- codex
//! cargo run --example probe            # every provider
//! PULSEWIN_DEBUG=1 cargo run --example probe -- claude-code
//! ```
//!
//! Unlike the unit tests (which parse fixed fixtures), this makes real network
//! calls with the credentials already on this machine.

use std::sync::Arc;

use pulsewin_lib::providers::{collect_all, registry, Ctx};

#[tokio::main]
async fn main() {
    let wanted = std::env::args().nth(1);

    let ctx = match Ctx::new() {
        Ok(ctx) => Arc::new(ctx),
        Err(e) => {
            eprintln!("could not build HTTP client: {e}");
            std::process::exit(1);
        }
    };

    let results = match &wanted {
        Some(id) => {
            let provider = registry().into_iter().find(|p| p.id() == id.as_str());
            match provider {
                Some(p) => vec![p.fetch(Arc::clone(&ctx)).await],
                None => {
                    let ids: Vec<&str> = registry().iter().map(|p| p.id()).collect();
                    eprintln!("unknown provider {id:?}; known: {}", ids.join(", "));
                    std::process::exit(2);
                }
            }
        }
        None => collect_all(ctx).await,
    };

    let mut failures = 0usize;

    for usage in &results {
        println!("\n=== {} ({}) ===", usage.name, usage.id);

        if let Some(plan) = &usage.plan {
            println!("  plan:    {plan}");
        }
        if let Some(account) = &usage.account {
            println!("  account: {account}");
        }

        if let Some(err) = &usage.error {
            failures += 1;
            println!("  ERROR:   {err}");
            continue;
        }

        for window in &usage.windows {
            let pct = window
                .percent_used
                .map(|p| format!("{p:.1}%"))
                .unwrap_or_else(|| "n/a".to_string());
            let reset = window.resets_at.as_deref().unwrap_or("-");
            let detail = window.detail.as_deref().unwrap_or("");
            println!(
                "  {:<12} used={:<8} resets={:<26} {}",
                window.label, pct, reset, detail
            );
        }
    }

    println!(
        "\n{} provider(s) checked, {} failed.",
        results.len(),
        failures
    );

    // A provider that is simply not installed is not a hard failure.
    if failures == results.len() && !results.is_empty() {
        std::process::exit(1);
    }
}

//! Pulse 1.9 OpenCodeConsolePager (Apache-2.0): bounded adaptive request-log walks.
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashSet, VecDeque},
    future::Future,
    sync::Arc,
    time::Duration,
};
use tokio::task::JoinSet;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Range {
    pub since: i64,
    pub until: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub started_at: f64,
    pub product: Option<String>,
    pub model: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub cost: Option<f64>,
}
impl Item {
    pub fn timestamp(&self) -> Option<i64> {
        (self.started_at.is_finite()
            && self.started_at > 0.
            && self.started_at < 9_007_199_254_740_992.)
            .then(|| self.started_at.round() as i64)
    }
    pub fn valid(&self) -> bool {
        !self.id.is_empty()
            && self.id.len() <= 1024
            && self.timestamp().is_some()
            && self
                .model
                .as_ref()
                .is_none_or(|s| s.len() <= 1024 && !s.chars().any(char::is_control))
            && self
                .product
                .as_ref()
                .is_none_or(|s| s.len() <= 64 && !s.chars().any(char::is_control))
            && self.cost.is_none_or(|v| v.is_finite() && v.abs() < 1e15)
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub items: Vec<Item>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug)]
pub enum Reply {
    Page(Page),
    SignedOut,
    Failed,
}
#[derive(Debug)]
pub struct Read {
    pub items: Vec<Item>,
    pub pending: Vec<Range>,
    pub signed_out: bool,
}
impl Read {
    pub fn complete(&self) -> bool {
        !self.signed_out && self.pending.is_empty()
    }
}
#[derive(Clone)]
struct Job {
    range: Range,
    cursor: Option<String>,
    seen: HashSet<String>,
}
fn split(range: &Range, page: &Page) -> Option<[Range; 2]> {
    let dates = page
        .items
        .iter()
        .map(Item::timestamp)
        .collect::<Option<Vec<_>>>()?;
    if !dates.iter().all(|d| *d >= range.since && *d <= range.until) {
        return None;
    }
    let oldest = *dates.iter().min()?;
    let newest = *dates.iter().max()?;
    let width = newest - oldest;
    if width < 1000 || oldest <= range.since || oldest >= range.until {
        return None;
    }
    let pivot = oldest.checked_sub(width)?;
    if pivot <= range.since || pivot >= oldest {
        return None;
    }
    Some([
        Range {
            since: pivot,
            until: oldest,
        },
        Range {
            since: range.since,
            until: pivot,
        },
    ])
}
#[cfg(test)]
pub async fn read<F, Fut>(ranges: Vec<Range>, limit: usize, retry_delay: Duration, fetch: F) -> Read
where
    F: Fn(Range, Option<String>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Reply> + Send + 'static,
{
    read_with_progress(ranges, limit, retry_delay, fetch, |_| {}).await
}
pub async fn read_with_progress<F, Fut, R>(
    ranges: Vec<Range>,
    limit: usize,
    retry_delay: Duration,
    fetch: F,
    receive: R,
) -> Read
where
    F: Fn(Range, Option<String>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Reply> + Send + 'static,
    R: Fn(&[Item]) + Send + Sync,
{
    let original = ranges.clone();
    let fetch = Arc::new(fetch);
    let mut queue = ranges
        .into_iter()
        .map(|range| Job {
            range,
            cursor: None,
            seen: HashSet::new(),
        })
        .collect::<VecDeque<_>>();
    let mut workers = JoinSet::new();
    let mut issued = 0usize;
    let mut result = Read {
        items: vec![],
        pending: vec![],
        signed_out: false,
    };
    loop {
        while workers.len() < 6 && issued < limit && !queue.is_empty() {
            let job = queue.pop_front().unwrap();
            issued += 1;
            let fetch = fetch.clone();
            workers.spawn(async move {
                let mut reply = Reply::Failed;
                for attempt in 0..3 {
                    if attempt > 0 {
                        tokio::time::sleep(retry_delay * attempt).await
                    }
                    reply = fetch(job.range.clone(), job.cursor.clone()).await;
                    if !matches!(reply, Reply::Failed) {
                        break;
                    }
                }
                (job, reply)
            });
        }
        if workers.is_empty() {
            break;
        }
        let Some(answer) = workers.join_next().await else {
            break;
        };
        let (mut job, reply) = match answer {
            Ok(value) => value,
            Err(_) => {
                workers.abort_all();
                result.pending = original;
                return result;
            }
        };
        match reply {
            Reply::SignedOut => {
                workers.abort_all();
                return Read {
                    items: vec![],
                    pending: original,
                    signed_out: true,
                };
            }
            Reply::Failed => result.pending.push(job.range),
            Reply::Page(page) => {
                if page.items.len() > 100 || page.items.iter().any(|i| !i.valid()) {
                    result.pending.push(job.range);
                    continue;
                }
                let split = split(&job.range, &page);
                let nonempty = !page.items.is_empty();
                receive(&page.items);
                result.items.extend(page.items);
                if let Some(cursor) = page.next_cursor.filter(|c| !c.is_empty()) {
                    if !nonempty || cursor.len() > 4096 || !job.seen.insert(cursor.clone()) {
                        result.pending.push(job.range);
                        continue;
                    }
                    if let Some(ranges) = split {
                        queue.extend(ranges.into_iter().map(|range| Job {
                            range,
                            cursor: None,
                            seen: HashSet::new(),
                        }))
                    } else {
                        job.cursor = Some(cursor);
                        queue.push_back(job)
                    }
                }
            }
        }
    }
    result.pending.extend(queue.into_iter().map(|j| j.range));
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };
    fn item(id: &str, at: f64) -> Item {
        Item {
            id: id.into(),
            started_at: at,
            product: Some("go".into()),
            model: Some("m".into()),
            input_tokens: Some(100),
            output_tokens: Some(20),
            cache_read_tokens: Some(30),
            cache_write_tokens: Some(5),
            cost: Some(0.01),
        }
    }
    #[tokio::test]
    async fn dense_intervals_split_with_inclusive_edges_and_keep_all_requests() {
        let calls = Arc::new(Mutex::new(vec![]));
        let record = calls.clone();
        let range = Range {
            since: 1,
            until: 10000,
        };
        let read = read(vec![range], 200, Duration::ZERO, move |range, cursor| {
            record.lock().unwrap().push((range.clone(), cursor));
            async move {
                if range.until == 10000 {
                    Reply::Page(Page {
                        items: vec![item("a", 9000.), item("b", 8000.)],
                        next_cursor: Some("more".into()),
                    })
                } else {
                    Reply::Page(Page {
                        items: vec![item("c", range.until as f64)],
                        next_cursor: None,
                    })
                }
            }
        })
        .await;
        assert!(read.complete());
        assert_eq!(read.items.len(), 4);
        let calls = calls.lock().unwrap();
        assert!(calls
            .iter()
            .any(|(r, _)| r.since == 7000 && r.until == 8000));
        assert!(calls.iter().any(|(r, _)| r.since == 1 && r.until == 7000));
    }
    #[tokio::test]
    async fn repeated_cursor_failed_range_and_page_budget_remain_pending() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let read = read(
            vec![Range {
                since: 1,
                until: 1000,
            }],
            200,
            Duration::ZERO,
            move |_, _| {
                count.fetch_add(1, Ordering::SeqCst);
                async {
                    Reply::Page(Page {
                        items: vec![item("burst", 900.)],
                        next_cursor: Some("same".into()),
                    })
                }
            },
        )
        .await;
        assert!(!read.complete());
        assert_eq!(read.pending.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let read = super::read(
            vec![Range {
                since: 1,
                until: 1000,
            }],
            1,
            Duration::ZERO,
            |_, _| async {
                Reply::Page(Page {
                    items: vec![item("a", 900.)],
                    next_cursor: Some("next".into()),
                })
            },
        )
        .await;
        assert_eq!(read.items.len(), 1);
        assert_eq!(read.pending.len(), 1);
        let read = super::read(
            vec![Range {
                since: 1,
                until: 1000,
            }],
            200,
            Duration::ZERO,
            |_, _| async { Reply::Failed },
        )
        .await;
        assert_eq!(read.pending.len(), 1);
    }
    #[tokio::test]
    async fn signed_out_discards_account_data_and_parallelism_is_bounded() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let a = active.clone();
        let p = peak.clone();
        let ranges = (1..20)
            .map(|n| Range {
                since: n * 1000,
                until: (n + 1) * 1000,
            })
            .collect::<Vec<_>>();
        let result = read(ranges, 200, Duration::ZERO, move |range, _| {
            let a = a.clone();
            let p = p.clone();
            async move {
                let n = a.fetch_add(1, Ordering::SeqCst) + 1;
                p.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                a.fetch_sub(1, Ordering::SeqCst);
                Reply::Page(Page {
                    items: vec![item("a", range.until as f64)],
                    next_cursor: None,
                })
            }
        })
        .await;
        assert!(result.complete());
        assert!(peak.load(Ordering::SeqCst) <= 6);
        assert!(peak.load(Ordering::SeqCst) > 1);
        let signed = read(
            vec![Range {
                since: 1,
                until: 1000,
            }],
            200,
            Duration::ZERO,
            |_, _| async { Reply::SignedOut },
        )
        .await;
        assert!(signed.signed_out);
        assert!(signed.items.is_empty());
        assert!(!signed.complete());
    }
    #[test]
    fn decoder_keeps_only_ledger_fields_and_invalid_page_is_not_empty_success() {
        let raw = r#"{"items":[{"id":"a","startedAt":1791705600000,"product":"go","inputTokens":100,"city":"PRIVATE_CITY","requestHeaders":{"authorization":"PRIVATE_KEY"}}],"nextCursor":null}"#;
        let page: Page = serde_json::from_str(raw).unwrap();
        assert!(page.items[0].valid());
        assert!(!serde_json::to_string(&page.items)
            .unwrap()
            .contains("PRIVATE"));
        assert!(serde_json::from_str::<Page>("{}").is_err());
        assert!(serde_json::from_str::<Page>(
            r#"{"items":[{"id":"a","startedAt":1,"inputTokens":-1}]}"#
        )
        .is_err());
    }
}

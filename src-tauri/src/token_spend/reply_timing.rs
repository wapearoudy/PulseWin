//! Logged request/response time only. Claude's first content block is not its first token.
use super::*;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyTiming {
    pub output_tokens: u64,
    pub seconds: f64,
    pub replies: u64,
    pub first_token_seconds: f64,
    pub first_token_turns: u64,
}
impl ReplyTiming {
    fn measured(output: u64, sent: i64, finished: i64) -> Option<Self> {
        let seconds = (finished - sent) as f64 / 1000.;
        (output >= 100 && seconds > 0. && seconds <= 600.).then_some(Self {
            output_tokens: output,
            seconds,
            replies: 1,
            ..Default::default()
        })
    }
    fn add(&mut self, other: &Self) {
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.seconds += other.seconds;
        self.replies += other.replies;
        self.first_token_seconds += other.first_token_seconds;
        self.first_token_turns += other.first_token_turns;
    }
}
#[derive(Default)]
pub(super) struct Clock {
    pending: Option<i64>,
    last_finished: Option<i64>,
    open: Option<(String, Option<i64>, i64, u64)>,
    done: HashSet<String>,
    pub timings: HashMap<String, ReplyTiming>,
    codex_sent: Option<i64>,
    codex_wrote: Option<i64>,
    closed: Option<(i64, i64)>,
    last_record: Option<usize>,
    last_ttft_turn: Option<String>,
}
impl Clock {
    pub fn observe(
        &mut self,
        root: &Value,
        agent: &str,
        replaying: bool,
        records: &mut [SpendRecord],
    ) {
        let Some(at) = root["timestamp"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.timestamp_millis())
        else {
            return;
        };
        if agent == "claude" {
            if root["type"] == "user" {
                self.pending = Some(at)
            }
            if root["type"] == "assistant" && root["message"]["model"] != "<synthetic>" {
                if let Some(id) = root["message"]["id"].as_str() {
                    let id = format!("claude:{id}");
                    let output = counter(&root["message"]["usage"]["output_tokens"]);
                    if let Some((open, _, finished, tokens)) = &mut self.open {
                        if open == &id {
                            *finished = at;
                            *tokens = (*tokens).max(output);
                            return;
                        }
                    }
                    self.finish();
                    if !self.done.contains(&id) {
                        let sent = self
                            .pending
                            .take()
                            .map(|t| self.last_finished.map(|f| t.max(f)).unwrap_or(t));
                        self.open = Some((id, sent, at, output));
                    }
                }
            }
        } else if agent == "codex" && !replaying {
            let p = &root["payload"];
            if root["type"] == "response_item" {
                let kind = p["type"].as_str().unwrap_or("");
                if p["role"] == "user" || kind.ends_with("_call_output") {
                    if let (Some(sent), Some(wrote)) = (self.codex_sent, self.codex_wrote) {
                        if wrote > sent {
                            self.closed = Some((sent, wrote))
                        }
                    }
                    self.codex_sent = Some(at);
                    self.codex_wrote = None;
                } else if p["role"] == "assistant"
                    || [
                        "reasoning",
                        "function_call",
                        "custom_tool_call",
                        "web_search_call",
                        "local_shell_call",
                    ]
                    .contains(&kind)
                {
                    if self.codex_sent.is_some() {
                        self.codex_wrote = Some(at)
                    }
                }
            }
            if p["type"] == "task_complete" {
                let turn = Some(
                    p["turn_id"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("timestamp:{at}")),
                );
                if turn != self.last_ttft_turn {
                    if let Some(seconds) = p["time_to_first_token_ms"]
                        .as_f64()
                        .map(|n| n / 1000.)
                        .filter(|n| *n > 0. && *n <= 120.)
                    {
                        if let Some(r) = self.last_record.and_then(|i| records.get_mut(i)) {
                            r.reply_timing
                                .get_or_insert_with(Default::default)
                                .add(&ReplyTiming {
                                    first_token_seconds: seconds,
                                    first_token_turns: 1,
                                    ..Default::default()
                                });
                            self.last_ttft_turn = turn;
                        }
                    }
                }
            }
        }
    }
    pub fn counted(&mut self, index: usize, record: &mut SpendRecord) {
        self.last_record = Some(index);
        let span = self.closed.take().or_else(|| {
            let span = self.codex_sent.zip(self.codex_wrote).filter(|(s, w)| w > s);
            if span.is_some() {
                self.codex_sent = None;
                self.codex_wrote = None
            }
            span
        });
        if let Some((sent, wrote)) = span {
            record.reply_timing = ReplyTiming::measured(record.tally.output, sent, wrote)
        }
    }
    pub fn finish(&mut self) {
        if let Some((id, sent, finished, output)) = self.open.take() {
            self.done.insert(id.clone());
            self.last_finished = Some(finished);
            if let Some(timing) =
                sent.and_then(|sent| ReplyTiming::measured(output, sent, finished))
            {
                self.timings.insert(id, timing);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speeds_require_nontrivial_output_and_bounded_positive_time() {
        assert!(ReplyTiming::measured(99, 0, 1000).is_none());
        assert!(ReplyTiming::measured(100, 1, 1).is_none());
        assert!(ReplyTiming::measured(100, 0, 600001).is_none());
        let m = ReplyTiming::measured(200, 0, 4000).unwrap();
        assert_eq!(m.output_tokens, 200);
        assert_eq!(m.seconds, 4.);
        assert_eq!(m.replies, 1);
    }
    #[test]
    fn claude_final_fragment_closes_from_request_not_first_block() {
        let mut clock = Clock::default();
        let mut records = [];
        for (kind, id, seconds, output) in [
            ("user", "", 0, 0),
            ("assistant", "reply", 1, 100),
            ("assistant", "reply", 4, 200),
        ] {
            clock.observe(&serde_json::json!({"type":kind,"timestamp":format!("2026-10-01T00:00:{seconds:02}Z"),"message":{"id":id,"model":"m","usage":{"output_tokens":output}}}),"claude",false,&mut records);
        }
        clock.finish();
        let m = &clock.timings["claude:reply"];
        assert_eq!(m.seconds, 4.);
        assert_eq!(m.output_tokens, 200);
        assert_eq!(m.first_token_turns, 0);
    }
}

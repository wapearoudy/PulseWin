//! Official, anonymous service-status readings. Ported from Pulse 1.9's
//! ServiceStatus/OutageMemory/StatusPage (Apache-2.0, copyright 2026 qunqin24).
//!
//! Every page is read from what its own status page is drawn from: a shared
//! feed per company, not per service. Cursor and Grok Bot both live on
//! status.cursor.com, Codex and the OpenAI Platform both on status.openai.com,
//! Kimi and Moonshot both on status.moonshot.cn — so the outage check reads a
//! company's feed once and asks each provider's rule what it runs on.
//!
//! Missing readings are errors; uptime is always the publisher's own figure;
//! a page that can't be read is no reading, never "all clear".

mod feeds;

use chrono::{DateTime, Days, Local, TimeZone, Utc};
use feeds::DaySpan;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use tauri::{Manager, State};

const INTERVAL: Duration = Duration::from_secs(300);
const MAX_BYTES: usize = 8 * 1024 * 1024;

// MARK: - What a page shows and what its provider runs on

/// Which rows of a page belong to one provider.
///
/// By id where the page has stable ids (names change, ids don't), by group
/// where rows come and go, by name only where nothing better exists.
#[derive(Debug, Clone, Copy)]
pub enum Rule {
    All,
    Ids(&'static [&'static str]),
    Groups(&'static [&'static str]),
    NameContains(&'static [&'static str]),
    Except(&'static Rule),
}
impl Rule {
    fn matches(&self, c: &Component) -> bool {
        match self {
            Self::All => true,
            Self::Ids(ids) => ids.contains(&c.id.as_str()),
            Self::Groups(groups) => c.group.as_deref().is_some_and(|g| groups.contains(&g)),
            Self::NameContains(parts) => parts.iter().any(|p| c.name.contains(p)),
            Self::Except(rule) => !rule.matches(c),
        }
    }
}

/// The twelve feed shapes. A feed belongs to the company, a page to the
/// provider; two providers on one feed share one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feed {
    Statuspage(&'static str),
    IncidentIo(&'static str),
    Flashcat(&'static str),
    BetterStack(&'static str),
    Instatus(&'static str),
    Upptime(&'static str),
    Gatus(&'static str),
    UptimeRobot(&'static str),
    OpenCode(&'static str),
    DeepInfra(&'static str),
    IbmBob(&'static str),
    GoogleCloud(&'static str, &'static str, &'static str),
}
impl Feed {
    /// The address of the one request that carries everything this feed has.
    fn url(self) -> String {
        match self {
            Self::Statuspage(root) => format!("{root}/api/v2/summary.json"),
            Self::IncidentIo(root) => format!("{}/proxy/{}", root, host_of(root)),
            Self::BetterStack(root) => format!("{root}/index.json"),
            Self::Instatus(root) => format!("{root}/v2/components.json"),
            Self::Flashcat(url)
            | Self::Upptime(url)
            | Self::Gatus(url)
            | Self::UptimeRobot(url)
            | Self::OpenCode(url)
            | Self::DeepInfra(url)
            | Self::IbmBob(url)
            | Self::GoogleCloud(url, _, _) => (*url).to_string(),
        }
    }
}

/// One provider's page.
#[derive(Debug, Clone, Copy)]
pub struct Page {
    pub provider: &'static str,
    /// The page a person opens.
    pub address: &'static str,
    pub company: &'static str,
    pub feed: Feed,
    /// The rows this pane draws.
    pub shows: Rule,
    /// Of those, what the provider's account actually runs on — the API, the
    /// agent a plan pays for, not marketing pages or docs.
    pub notifies: Rule,
    pub day_count: u32,
}
impl Page {
    pub fn id(&self) -> &'static str {
        self.provider
    }
    pub fn host(&self) -> String {
        host_of(self.address)
    }
    /// The rows whose outage is news for this provider.
    pub fn notifies_about(&self, c: &Component) -> bool {
        self.shows.matches(c) && self.notifies.matches(c)
    }
    pub fn palette(&self) -> Palette {
        match self.feed {
            Feed::OpenCode(_) => Palette::OpenCode,
            _ => match self.host().as_str() {
                "status.openai.com" => Palette::OpenAi,
                "status.claude.com" => Palette::Claude,
                "status.cursor.com" => Palette::Cursor,
                _ => match self.feed {
                    Feed::Statuspage(_) => Palette::Statuspage,
                    Feed::IncidentIo(_) => Palette::IncidentIo,
                    _ => Palette::Default,
                },
            },
        }
    }
}

pub fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .to_string()
}

// MARK: - The catalogue

const EXCEPT_CURSOR_HIDDEN: Rule = Rule::Except(&Rule::Ids(&["sm5wkcnqkvr9"]));
const EXCEPT_DEVIN_GROUPS: Rule =
    Rule::Except(&Rule::Groups(&["92j5h0ttqxfb", "9ydbk2b55pk1"]));
const EXCEPT_FACTORY_GROUP: Rule = Rule::Except(&Rule::Groups(&["c9sjq8nsz2kq"]));
const EXCEPT_KILO_GROUP: Rule = Rule::Except(&Rule::Groups(&["frgc645htq00"]));
const EXCEPT_JETBRAINS_GROUP: Rule = Rule::Except(&Rule::Groups(&["AI Providers"]));
const EXCEPT_ZED_GROUP: Rule = Rule::Except(&Rule::Groups(&["Delta"]));

static CATALOGUE: &[Page] = &[
    // Atlassian Statuspage
    Page {
        provider: "claude-code",
        address: "https://status.claude.com",
        company: "Anthropic",
        feed: Feed::Statuspage("https://status.claude.com"),
        shows: Rule::All,
        notifies: Rule::Ids(&["yyzkbfz2thpt", "k8w3r06qmzrp"]),
        day_count: 90,
    },
    Page {
        provider: "cursor",
        address: "https://status.cursor.com",
        company: "Cursor",
        feed: Feed::Statuspage("https://status.cursor.com"),
        shows: EXCEPT_CURSOR_HIDDEN,
        notifies: Rule::Ids(&["rflc60xp5jp2", "vsny1qv7v86c", "mwv1g9sc7kdh"]),
        day_count: 90,
    },
    Page {
        provider: "grok-bot",
        address: "https://status.cursor.com",
        company: "Cursor",
        feed: Feed::Statuspage("https://status.cursor.com"),
        shows: Rule::Ids(&["sm5wkcnqkvr9"]),
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "copilot",
        address: "https://www.githubstatus.com",
        company: "GitHub",
        feed: Feed::Statuspage("https://www.githubstatus.com"),
        shows: Rule::Ids(&["pjmpxvq2cmr2", "cnnb39dkkk82"]),
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "devin",
        address: "https://www.devinstatus.com",
        company: "Cognition",
        feed: Feed::Statuspage("https://www.devinstatus.com"),
        shows: EXCEPT_DEVIN_GROUPS,
        notifies: Rule::Ids(&["q72cy1kjpk4r", "d87cp5jknh1c"]),
        day_count: 90,
    },
    Page {
        provider: "windsurf",
        address: "https://status.windsurf.com",
        company: "Windsurf",
        feed: Feed::Statuspage("https://status.windsurf.com"),
        shows: Rule::Ids(&["r5wf1ykd7y1m", "8q19cygxvshj"]),
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "augment",
        address: "https://status.augmentcode.com",
        company: "Augment Code",
        feed: Feed::Statuspage("https://status.augmentcode.com"),
        shows: Rule::All,
        notifies: Rule::Ids(&["1k3gvg4j889s"]),
        day_count: 90,
    },
    Page {
        provider: "factory",
        address: "https://status.factory.ai",
        company: "Factory",
        feed: Feed::Statuspage("https://status.factory.ai"),
        shows: EXCEPT_FACTORY_GROUP,
        notifies: Rule::Ids(&["kfqc4mfdkrqx", "jfcmwr3grbyc", "z1frly8fm1l5"]),
        day_count: 90,
    },
    Page {
        provider: "kilo-code",
        address: "https://status.kilo.ai",
        company: "Kilo",
        feed: Feed::Statuspage("https://status.kilo.ai"),
        shows: EXCEPT_KILO_GROUP,
        notifies: Rule::Ids(&["kdk5t7ljzmm1", "rkvq7kvz2zlh"]),
        day_count: 90,
    },
    Page {
        provider: "minimax",
        address: "https://status.minimax.io",
        company: "MiniMax",
        feed: Feed::Statuspage("https://status.minimax.io"),
        shows: Rule::All,
        notifies: Rule::Ids(&["pr0d8qr59svt"]),
        day_count: 90,
    },
    Page {
        provider: "minimax-cn",
        address: "https://status.minimaxi.com",
        company: "MiniMax",
        feed: Feed::Statuspage("https://status.minimaxi.com"),
        shows: Rule::All,
        notifies: Rule::Ids(&["vwp8mgy34fck"]),
        day_count: 90,
    },
    Page {
        provider: "kimi-code",
        address: "https://status.moonshot.cn",
        company: "Moonshot AI",
        feed: Feed::Statuspage("https://status.moonshot.cn"),
        shows: Rule::Groups(&["cyrtlddz0q8c"]),
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "moonshot",
        address: "https://status.moonshot.cn",
        company: "Moonshot AI",
        feed: Feed::Statuspage("https://status.moonshot.cn"),
        shows: Rule::Groups(&["87jpctq5cp71"]),
        notifies: Rule::Ids(&["8psr5dfdld0s", "pjrph68g94yr"]),
        day_count: 90,
    },
    Page {
        provider: "warp",
        address: "https://status.warp.dev",
        company: "Warp",
        feed: Feed::Statuspage("https://status.warp.dev"),
        shows: Rule::All,
        notifies: Rule::Ids(&["z7hlxwbqx7ry", "z90zfbttkx9f", "7z5yq9wznb4f"]),
        day_count: 90,
    },
    Page {
        provider: "manus",
        address: "https://status.manus.im",
        company: "Manus",
        feed: Feed::Statuspage("https://status.manus.im"),
        shows: Rule::All,
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "venice",
        address: "https://veniceai-status.com",
        company: "Venice",
        feed: Feed::Statuspage("https://veniceai-status.com"),
        shows: Rule::All,
        notifies: Rule::Ids(&["6knqw5vybqzk"]),
        day_count: 90,
    },
    Page {
        provider: "zoom-mate",
        address: "https://www.zoomstatus.com",
        company: "Zoom",
        feed: Feed::Statuspage("https://www.zoomstatus.com"),
        shows: Rule::Ids(&["n1llzwc1cxtr"]),
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "replicate",
        address: "https://www.cloudflarestatus.com",
        company: "Cloudflare",
        feed: Feed::Statuspage("https://www.cloudflarestatus.com"),
        shows: Rule::Ids(&["fvgfcmy66tdr"]),
        notifies: Rule::All,
        day_count: 90,
    },
    // incident.io
    Page {
        provider: "codex",
        address: "https://status.openai.com",
        company: "OpenAI",
        feed: Feed::IncidentIo("https://status.openai.com"),
        shows: Rule::Groups(&["Codex"]),
        notifies: Rule::All,
        day_count: 91,
    },
    Page {
        provider: "openai-api",
        address: "https://status.openai.com",
        company: "OpenAI",
        feed: Feed::IncidentIo("https://status.openai.com"),
        shows: Rule::Groups(&["APIs"]),
        notifies: Rule::All,
        day_count: 91,
    },
    Page {
        provider: "jetbrains-ai",
        address: "https://status.jetbrains.cloud",
        company: "JetBrains",
        feed: Feed::IncidentIo("https://status.jetbrains.cloud"),
        shows: EXCEPT_JETBRAINS_GROUP,
        notifies: Rule::Ids(&["01KX3EN535A0SKSZK3S84949V1"]),
        day_count: 90,
    },
    Page {
        provider: "amp",
        address: "https://ampcodestatus.com",
        company: "Amp",
        feed: Feed::IncidentIo("https://ampcodestatus.com"),
        shows: Rule::Ids(&[
            "01KBQ3ZK9NMBWKR7DY9H7NQ17D",
            "01M0WN9KPF89TDEHSPQ918XHTH",
        ]),
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "aixy",
        address: "https://status.aixy-gateway.com",
        company: "AIXY",
        feed: Feed::IncidentIo("https://status.aixy-gateway.com"),
        shows: Rule::All,
        notifies: Rule::Ids(&["01M08789J59XQG5AR373SVQZ6Z"]),
        day_count: 90,
    },
    Page {
        provider: "elevenlabs",
        address: "https://status.elevenlabs.io",
        company: "ElevenLabs",
        feed: Feed::IncidentIo("https://status.elevenlabs.io"),
        shows: Rule::Groups(&["ElevenAPI"]),
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "notion-ai",
        address: "https://www.notion-status.com",
        company: "Notion",
        feed: Feed::IncidentIo("https://www.notion-status.com"),
        shows: Rule::Groups(&["Notion AI"]),
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "perplexity",
        address: "https://status.perplexity.com",
        company: "Perplexity",
        feed: Feed::IncidentIo("https://status.perplexity.com"),
        shows: Rule::All,
        notifies: Rule::Ids(&["01M2R6E5FPASTEW3A3V8TSYHJ0"]),
        day_count: 90,
    },
    Page {
        provider: "poe",
        address: "https://status.poe.com",
        company: "Poe",
        feed: Feed::IncidentIo("https://status.poe.com"),
        shows: Rule::All,
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "v0",
        address: "https://v0-status.com",
        company: "Vercel",
        feed: Feed::IncidentIo("https://v0-status.com"),
        shows: Rule::All,
        notifies: Rule::Ids(&[
            "01KHYYTCKK0NS8FN857PCJB63W",
            "01J5EED92VTMVYNWYKYS9B1G1F",
        ]),
        day_count: 90,
    },
    // Flashcat
    Page {
        provider: "deepseek",
        address: "https://status.deepseek.com",
        company: "DeepSeek",
        feed: Feed::Flashcat("https://status.deepseek.com/"),
        shows: Rule::All,
        notifies: Rule::NameContains(&["API"]),
        day_count: 90,
    },
    // Better Stack
    Page {
        provider: "hugging-face",
        address: "https://status.huggingface.co",
        company: "Hugging Face",
        feed: Feed::BetterStack("https://status.huggingface.co"),
        shows: Rule::All,
        notifies: Rule::Ids(&["306621", "84641"]),
        day_count: 90,
    },
    Page {
        provider: "chutes",
        address: "https://status.chutes.ai",
        company: "Chutes",
        feed: Feed::BetterStack("https://status.chutes.ai"),
        shows: Rule::All,
        notifies: Rule::Ids(&["8599285"]),
        day_count: 90,
    },
    Page {
        provider: "synthetic",
        address: "https://status.synthetic.new",
        company: "Synthetic",
        feed: Feed::BetterStack("https://status.synthetic.new"),
        shows: Rule::All,
        notifies: Rule::Groups(&["Synthetic - API (Synthetic hosted Models)"]),
        day_count: 90,
    },
    Page {
        provider: "dev-pass",
        address: "https://status.llmgateway.io",
        company: "LLM Gateway",
        feed: Feed::BetterStack("https://status.llmgateway.io"),
        shows: Rule::All,
        notifies: Rule::Ids(&["8768001"]),
        day_count: 90,
    },
    Page {
        provider: "raycast-ai",
        address: "https://status.raycast.com",
        company: "Raycast",
        feed: Feed::BetterStack("https://status.raycast.com"),
        shows: Rule::All,
        notifies: Rule::Ids(&["5341215"]),
        day_count: 90,
    },
    Page {
        provider: "typesafe",
        address: "https://status.typesafe.ai",
        company: "TypeSafe",
        feed: Feed::BetterStack("https://status.typesafe.ai"),
        shows: Rule::All,
        notifies: Rule::Ids(&["8735312"]),
        day_count: 90,
    },
    // Instatus
    Page {
        provider: "zed",
        address: "https://status.zed.dev",
        company: "Zed",
        feed: Feed::Instatus("https://status.zed.dev"),
        shows: EXCEPT_ZED_GROUP,
        notifies: Rule::Ids(&["cmaw974wo0001xh7v9vfihlul", "cm75bq8t9000rsdcb9bfls8wt"]),
        day_count: 90,
    },
    Page {
        provider: "gitkraken",
        address: "https://cloud.gitkrakenstatus.com",
        company: "GitKraken",
        feed: Feed::Instatus("https://cloud.gitkrakenstatus.com"),
        shows: Rule::All,
        notifies: Rule::Ids(&["clvkz1ojq2917bgo9peyj3bha", "clvkz1ok12924bgo956mkaa0r"]),
        day_count: 90,
    },
    Page {
        provider: "atlas-cloud",
        address: "https://atlascloud.instatus.com",
        company: "Atlas Cloud",
        feed: Feed::Instatus("https://atlascloud.instatus.com"),
        shows: Rule::All,
        notifies: Rule::All,
        day_count: 90,
    },
    // Monitors and companies' own pages
    Page {
        provider: "command-code",
        address: "https://status.commandcode.ai",
        company: "Command Code",
        feed: Feed::Upptime(
            "https://raw.githubusercontent.com/CommandCodeAI/commandcode-status/master/history/summary.json",
        ),
        shows: Rule::All,
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "cline-pass",
        address: "https://status.cline.bot",
        company: "Cline",
        feed: Feed::Gatus("https://status.cline.bot/api/v1/endpoints/statuses"),
        shows: Rule::All,
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "hyper",
        address: "https://status.charm.sh",
        company: "Charm",
        feed: Feed::UptimeRobot(
            "https://status.charm.sh/api/getMonitorList/JrppyTBzE3?page=1",
        ),
        shows: Rule::Ids(&["803945156"]),
        notifies: Rule::All,
        day_count: 90,
    },
    Page {
        provider: "opencode-go",
        address: "https://status.opencode.ai",
        company: "OpenCode",
        feed: Feed::OpenCode("https://status.opencode.ai/api/status"),
        shows: Rule::All,
        notifies: Rule::Ids(&["go"]),
        day_count: 90,
    },
    Page {
        provider: "deepinfra",
        address: "https://status.deepinfra.com",
        company: "DeepInfra",
        feed: Feed::DeepInfra("https://status.deepinfra.com/api/v1/status"),
        shows: Rule::All,
        notifies: Rule::Ids(&["api"]),
        day_count: 90,
    },
    Page {
        provider: "ibm-bob",
        address: "https://status.bob.ibm.com",
        company: "IBM",
        feed: Feed::IbmBob("https://status.bob.ibm.com/api/v1/status"),
        shows: Rule::All,
        notifies: Rule::Ids(&["Chat/Code Completion", "Login/SSO"]),
        day_count: 90,
    },
    Page {
        provider: "gemini-cli",
        address: "https://status.cloud.google.com",
        company: "Google Cloud",
        feed: Feed::GoogleCloud(
            "https://status.cloud.google.com/incidents.json",
            "deUeOEPYanfJ9w8cpyBJ",
            "Gemini Code Assist",
        ),
        shows: Rule::All,
        notifies: Rule::All,
        day_count: 90,
    },
];

/// Every provider with a page, in the original's order.
pub fn catalogue() -> &'static [Page] {
    CATALOGUE
}
/// The page one provider id reads, if it has one.
///
/// Providers with no page are deliberate: xAI's and Mistral's status sit behind
/// a bot check, and a stand-in page would say "all clear" about nothing.
pub fn page(provider: &str) -> Option<&'static Page> {
    CATALOGUE.iter().find(|p| p.provider == provider)
}
/// The provider ids that have a page, for the settings pane to gate on.
///
/// The pane doesn't keep its own copy of this list: which services publish a
/// page is the catalogue's fact, and a second copy would drift.
#[tauri::command]
pub fn status_pages() -> Vec<&'static str> {
    CATALOGUE.iter().map(|p| p.provider).collect()
}

// MARK: - Reading a page

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Health {
    Operational,
    Maintenance,
    Unrecognised,
    Degraded,
    PartialOutage,
    FullOutage,
}
impl Health {
    /// A feed's word for a state. Case and punctuation carry no meaning
    /// (`DEGRADEDPERFORMANCE`, `degraded_performance`, `Degraded performance`
    /// are one state), so only letters are compared.
    pub fn parse(s: &str) -> Self {
        let word = s
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .collect::<String>();
        match word.as_str() {
            "operational" => Self::Operational,
            "degradedperformance" | "degraded" => Self::Degraded,
            "partialoutage" => Self::PartialOutage,
            "fulloutage" | "majoroutage" | "outage" => Self::FullOutage,
            "undermaintenance" | "maintenance" => Self::Maintenance,
            _ => Self::Unrecognised,
        }
    }
    pub fn severity(self) -> u8 {
        match self {
            Self::Operational => 0,
            Self::Maintenance => 1,
            Self::Unrecognised => 2,
            Self::Degraded => 3,
            Self::PartialOutage => 4,
            Self::FullOutage => 5,
        }
    }
    /// An outage is something going wrong now. Maintenance is planned and
    /// unrecognised was never witnessed, so neither is one.
    fn outage(self) -> bool {
        matches!(self, Self::Degraded | Self::PartialOutage | Self::FullOutage)
    }
    /// The name of the state in a notification's own words.
    fn phrase(self) -> &'static str {
        match self {
            Self::Operational => "运行正常",
            Self::Maintenance => "计划维护",
            Self::Unrecognised => "状态未知",
            Self::Degraded => "性能下降",
            Self::PartialOutage => "部分故障",
            Self::FullOutage => "全面故障",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    /// `YYYY-MM-DD`; when a bar is shorter than a day it also carries the hour.
    date: String,
    state: Option<Health>,
    /// The page's own drawn colour, `#rrggbb`, when it publishes one.
    colour: Option<String>,
    /// Bar length in hours when the page draws sub-day bars (OpenCode: 3).
    hours: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub id: String,
    pub name: String,
    pub state: Health,
    days: Vec<Day>,
    uptime: Option<f64>,
    /// What a group rule matches on.
    group: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Palette {
    OpenCode,
    OpenAi,
    Claude,
    Cursor,
    Statuspage,
    IncidentIo,
    Default,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    /// The provider this page belongs to; a company's page serves several.
    page: String,
    /// The page a person opens.
    address: String,
    company: String,
    palette: Palette,
    components: Vec<Component>,
    checked_at: String,
    history_error: Option<String>,
}

fn component(id: &str, name: &str, state: Health) -> Component {
    Component {
        id: id.into(),
        name: name.into(),
        state,
        days: vec![],
        uptime: None,
        group: None,
    }
}

/// The last `count` local calendar days, oldest first, today last.
fn calendar(now: DateTime<Local>, count: u32) -> Vec<DaySpan> {
    (0..count)
        .rev()
        .filter_map(|back| {
            let date = now.date_naive().checked_sub_days(Days::new(back as u64))?;
            let next = date.checked_add_days(Days::new(1))?;
            // Earliest local midnight handles repeated DST boundaries; absent
            // midnight is omitted, never silently replaced with a 24-hour UTC day.
            let a = Local
                .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
                .earliest()?;
            let b = Local
                .from_local_datetime(&next.and_hms_opt(0, 0, 0)?)
                .earliest()?;
            Some((date.to_string(), a.timestamp(), b.timestamp()))
        })
        .collect()
}

fn nonempty(rows: Vec<Component>) -> Result<Vec<Component>, String> {
    if rows.is_empty() {
        Err("官方状态页没有可读取的组件".into())
    } else {
        Ok(rows)
    }
}

// MARK: - Network

async fn fetch(
    client: &reqwest::Client,
    url: &str,
    query: &[(&str, String)],
    html: bool,
) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .query(query)
        .header(
            "Accept",
            if html {
                "text/html"
            } else {
                "application/json"
            },
        )
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| "官方状态页请求失败".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "官方状态页返回 HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("官方状态数据超过大小限制".into());
    }
    let mut data = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "官方状态响应未完整读取")?
    {
        if data.len() + chunk.len() > MAX_BYTES {
            return Err("官方状态数据超过大小限制".into());
        }
        data.extend_from_slice(&chunk)
    }
    Ok(data)
}
async fn json(
    client: &reqwest::Client,
    url: &str,
    query: &[(&str, String)],
) -> Result<Value, String> {
    serde_json::from_slice(&fetch(client, url, query, false).await?)
        .map_err(|_| "官方状态页返回了无法识别的数据".into())
}

/// The state-only read every feed that cannot cite more than one request needs.
///
/// One request per feed serves the whole company: Cursor and Grok Bot, Codex
/// and the OpenAI Platform, Kimi and Moonshot each share one.
async fn current(client: &reqwest::Client, feed: Feed) -> Option<Vec<Component>> {
    let v = json(client, &feed.url(), &[]).await.ok()?;
    match feed {
        Feed::Statuspage(_) => feeds::statuspage_components(&v),
        Feed::IncidentIo(_) => feeds::incidentio_summary(&v).map(|(c, _)| c),
        Feed::BetterStack(_) => feeds::better_stack(&v),
        Feed::Instatus(_) => feeds::instatus(&v),
        Feed::Upptime(_) => feeds::upptime(&v),
        Feed::Gatus(_) => feeds::gatus(&v),
        Feed::UptimeRobot(_) => feeds::uptime_robot(&v),
        Feed::OpenCode(_) => feeds::open_code(&v, Utc::now().timestamp()),
        Feed::DeepInfra(_) => feeds::deep_infra(&v),
        Feed::IbmBob(_) => feeds::ibm_bob(&v),
        Feed::GoogleCloud(_, product, name) => feeds::google_cloud(
            &v,
            product,
            name,
            Utc::now().timestamp(),
            None,
        ),
        // These need the page's own payload, not JSON: see `read`.
        Feed::Flashcat(_) => None,
    }
}

async fn read_statuspage(
    client: &reqwest::Client,
    page: &Page,
    root: &str,
    history: bool,
) -> Result<(Vec<Component>, Option<String>), String> {
    let summary = json(client, &format!("{root}/api/v2/summary.json"), &[]).await?;
    let mut rows = feeds::statuspage_components(&summary)
        .ok_or_else(|| "官方状态页没有可读取的组件".to_string())?;
    rows.retain(|c| page.shows.matches(c));
    let mut rows = nonempty(rows)?;
    let mut history_error = None;
    if history {
        let query = [(
            "components",
            rows.iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>()
                .join(","),
        )];
        // A page whose showcase is missing (Zoom draws no bars, Cloudflare's
        // answers 404) still shows its states: no bars is not an error.
        match json(client, &format!("{root}/uptime_showcase"), &query).await {
            Ok(v) => history_error = feeds::statuspage_history(&v, &mut rows).err(),
            Err(e) => history_error = Some(e),
        }
    }
    Ok((rows, history_error))
}

async fn read_incident_io(
    client: &reqwest::Client,
    page: &Page,
    root: &str,
    days: &[DaySpan],
    now: i64,
    history: bool,
) -> Result<(Vec<Component>, Option<String>), String> {
    let url = format!("{}/proxy/{}", root, host_of(root));
    let summary = json(client, &url, &[]).await?;
    let (rows, shows_uptime) =
        feeds::incidentio_summary(&summary).ok_or("官方状态页没有可读取的组件")?;
    let mut rows = nonempty(
        rows.into_iter()
            .filter(|c| page.shows.matches(c))
            .collect(),
    )?;
    let mut history_error = None;
    if history {
        if let (Some(first), Some(last)) = (days.first(), days.last()) {
            let iso = |s: i64| DateTime::from_timestamp(s, 0).unwrap().to_rfc3339();
            let query = [
                ("start_at", iso(first.1)),
                ("end_at", iso(last.2)),
            ];
            match json(client, &format!("{url}/component_impacts"), &query).await {
                Ok(v) => {
                    history_error = feeds::impact_history(&mut rows, &v, days, now, false).err()
                }
                Err(e) => history_error = Some(e),
            }
        }
    }
    // The feed carries a figure; a page set to draw bars without a percentage
    // does not show it, so neither do we.
    if !shows_uptime {
        for c in rows.iter_mut() {
            c.uptime = None;
        }
    }
    Ok((rows, history_error))
}

/// Read one page. `history` false is the outage check's path: state only.
pub async fn read(
    client: &reqwest::Client,
    provider: &str,
    history: bool,
) -> Result<Reading, String> {
    let page = page(provider).ok_or("此服务没有受支持的官方状态页")?;
    let now = Local::now();
    let now_ts = now.timestamp();
    let days = calendar(now, page.day_count);
    let (rows, history_error) = match page.feed {
        Feed::Statuspage(root) => read_statuspage(client, page, root, history).await?,
        Feed::IncidentIo(root) => {
            read_incident_io(client, page, root, &days, now_ts, history).await?
        }
        Feed::Flashcat(url) => {
            let bytes = fetch(client, url, &[], true).await?;
            let v = feeds::flashcat_data(&String::from_utf8_lossy(&bytes))?;
            let mut rows = nonempty(
                feeds::flashcat_summary(&v, now_ts)?
                    .into_iter()
                    .filter(|c| page.shows.matches(c))
                    .collect(),
            )?;
            let mut history_error = None;
            if history {
                history_error =
                    feeds::impact_history(&mut rows, &v, &days, now_ts, true).err();
            }
            (rows, history_error)
        }
        Feed::GoogleCloud(_, product, name) => {
            let v = json(client, page.feed.url().as_str(), &[]).await?;
            let rows = nonempty(
                feeds::google_cloud(&v, product, name, now_ts, Some(&days))
                    .ok_or("官方状态页没有可读取的组件")?
                    .into_iter()
                    .filter(|c| page.shows.matches(c))
                    .collect(),
            )?;
            (rows, None)
        }
        feed => {
            let rows = current(client, feed)
                .await
                .ok_or_else(|| "官方状态页没有可读取的组件".to_string())?;
            (nonempty(rows.into_iter().filter(|c| page.shows.matches(c)).collect())?, None)
        }
    };
    Ok(Reading {
        page: page.provider.to_string(),
        address: page.address.to_string(),
        company: page.company.to_string(),
        palette: page.palette(),
        components: rows,
        checked_at: Utc::now().to_rfc3339(),
        history_error,
    })
}

type Cache = HashMap<(String, bool), (Instant, Result<Reading, String>)>;
#[derive(Default)]
pub struct StatusState {
    cache: tokio::sync::Mutex<Cache>,
    memory: std::sync::Mutex<OutageMemory>,
    wake: tokio::sync::Notify,
}
impl StatusState {
    async fn reading(
        &self,
        client: &reqwest::Client,
        provider: &str,
        history: bool,
    ) -> Result<Reading, String> {
        // One in-flight public request per cache key. Failures are cached too;
        // opening a pane repeatedly cannot hammer a publisher or show stale green.
        let key = (provider.to_string(), history);
        let mut cache = self.cache.lock().await;
        if let Some((at, result)) = cache.get(&key) {
            if at.elapsed() < INTERVAL {
                return result.clone();
            }
        }
        if !history {
            if let Some((at, Ok(result))) = cache.get(&(provider.to_string(), true)) {
                if at.elapsed() < INTERVAL {
                    return Ok(result.clone());
                }
            }
        }
        let asked = Instant::now();
        let result = read(client, provider, history).await;
        cache.insert(key, (asked, result.clone()));
        result
    }
}

#[tauri::command]
pub async fn get_service_status(
    state: State<'_, crate::AppState>,
    status: State<'_, StatusState>,
    provider: String,
) -> Result<Reading, String> {
    let prefs = state.preferences.lock().unwrap().clone();
    let base = prefs
        .accounts
        .iter()
        .find(|a| a.id == provider)
        .map(|a| a.provider.as_str())
        .unwrap_or(&provider);
    let page = page(base).ok_or("此服务没有受支持的官方状态页")?;
    let ctx = crate::providers::Ctx::current(&state.ctx, &prefs.network_proxy)
        .map_err(|_| "无法使用当前代理访问官方状态页")?;
    status.reading(&ctx.client, page.provider, true).await
}

// MARK: - Notifications

/// What the user has already been told about each provider's page.
///
/// Kept in its own file (`status-alerts.json`), deliberately not a field on the
/// limit-warning memory: that type decodes as a whole, so a missing key would
/// throw away every limit already warned about.
#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
struct OutageMemory {
    /// Provider id → component id → the state last announced or recorded.
    announced: HashMap<String, HashMap<String, Health>>,
}
struct Change {
    worse: Vec<Component>,
    recovered: Vec<Component>,
}
impl OutageMemory {
    /// What is news, and what to remember. Pure: no clock, disk or window.
    fn changes(&mut self, provider: &str, components: &[Component]) -> Change {
        let before = self.announced.remove(provider).unwrap_or_default();
        let mut next = HashMap::new();
        let mut change = Change {
            worse: vec![],
            recovered: vec![],
        };
        for c in components {
            let old = before.get(&c.id).copied();
            if c.state.outage() {
                if old.is_none_or(|s| c.state.severity() > s.severity()) {
                    change.worse.push(c.clone());
                }
                // Still down but no worse is recorded without a word, so that
                // getting worse again is news again.
                next.insert(c.id.clone(), c.state);
            } else if c.state == Health::Operational {
                if old.is_some() {
                    change.recovered.push(c.clone());
                }
            } else if let Some(old) = old {
                next.insert(c.id.clone(), old);
            }
        }
        if next.is_empty() {
            self.announced.remove(provider);
        } else {
            self.announced.insert(provider.to_string(), next);
        }
        change
    }
    fn keep(&mut self, providers: &HashSet<String>) {
        self.announced.retain(|p, _| providers.contains(p));
    }
    fn load() -> Self {
        crate::settings::directory()
            .and_then(|d| std::fs::read(d.join("status-alerts.json")).ok())
            .filter(|b| b.len() < 1024 * 1024)
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn save(&self) -> Result<(), String> {
        let d = crate::settings::directory().ok_or("无法保存服务通知记录")?;
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        let path = d.join("status-alerts.json");
        let old = std::fs::read(&path).ok();
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        crate::credential_store::atomic_write_bounded(&path, &bytes, old.as_deref(), 1024 * 1024)
            .map_err(|e| e.to_string())
    }
}

/// The providers whose pages are watched right now, in the original's order.
fn watched(prefs: &crate::preferences::Preferences) -> Vec<&'static Page> {
    if !prefs.alerts.on_service_outage {
        return vec![];
    }
    let mut pages = prefs
        .enabled_providers
        .iter()
        .filter_map(|id| {
            let base = prefs
                .accounts
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.provider.as_str())
                .unwrap_or(id);
            page(base)
        })
        .collect::<Vec<_>>();
    pages.sort_by_key(|p| p.provider);
    pages.dedup_by_key(|p| p.provider);
    pages
}

pub fn preferences_changed(app: &tauri::AppHandle) {
    let state = app.state::<crate::AppState>();
    let prefs = state.preferences.lock().unwrap().clone();
    let watching = watched(&prefs);
    let ids = watching
        .iter()
        .map(|p| p.provider.to_string())
        .collect::<HashSet<_>>();
    let status = app.state::<StatusState>();
    let mut memory = status.memory.lock().unwrap();
    memory.keep(&ids);
    let _ = memory.save();
    status.wake.notify_one();
}

/// One notification per provider, and never about an outage nobody heard of.
fn notice(page: &Page, change: &Change) -> Option<String> {
    let list = |rows: &[Component], states: bool| {
        rows.iter()
            .map(|c| {
                if states {
                    format!("{}（{}）", c.name, c.state.phrase())
                } else {
                    c.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("、")
    };
    let mut parts = vec![];
    if !change.worse.is_empty() {
        parts.push(format!(
            "{} 报告：{}。",
            page.company,
            list(&change.worse, true)
        ));
    }
    if !change.recovered.is_empty() {
        parts.push(format!("恢复正常：{}。", list(&change.recovered, false)));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

pub fn watch(app: tauri::AppHandle) {
    *app.state::<StatusState>().memory.lock().unwrap() = OutageMemory::load();
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app.state::<crate::AppState>();
            let status = app.state::<StatusState>();
            let prefs = state.preferences.lock().unwrap().clone();
            let watching = watched(&prefs);
            {
                let mut memory = status.memory.lock().unwrap();
                memory.keep(
                    &watching
                        .iter()
                        .map(|p| p.provider.to_string())
                        .collect::<HashSet<_>>(),
                );
                let _ = memory.save();
            }
            // A company's page is read once for every provider on it.
            let mut feeds = watching
                .iter()
                .map(|p| p.feed)
                .collect::<Vec<_>>();
            feeds.sort_by_key(|f| (*f).url());
            feeds.dedup();
            for feed in feeds {
                let ctx = match crate::providers::Ctx::current(&state.ctx, &prefs.network_proxy) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let Some(components) = current(&ctx.client, feed).await else {
                    continue;
                };
                for page in watching.iter().filter(|p| p.feed == feed) {
                    // Recheck after the network await: disabling a provider or
                    // the alert must stop a read already under way.
                    let live = {
                        let current = state.preferences.lock().unwrap().clone();
                        let current_pages = watched(&current);
                        let still = current_pages.iter().any(|p| p.provider == page.provider);
                        let ctx_ok = current.network_proxy == prefs.network_proxy;
                        still && ctx_ok
                    };
                    if !live {
                        continue;
                    }
                    let shown = components
                        .iter()
                        .filter(|c| page.notifies_about(c))
                        .cloned()
                        .collect::<Vec<_>>();
                    let mut memory = status.memory.lock().unwrap();
                    let old = memory.clone();
                    let change = memory.changes(page.provider, &shown);
                    use tauri_plugin_notification::NotificationExt;
                    let mut delivered = true;
                    if let Some(body) = notice(page, &change) {
                        if app
                            .notification()
                            .builder()
                            .title(format!("Pulse · {} · 服务状态", page.provider))
                            .body(body)
                            .show()
                            .is_err()
                        {
                            delivered = false
                        }
                    }
                    if !delivered {
                        *memory = old;
                    }
                    let _ = memory.save();
                }
            }
            tokio::select! {_=tokio::time::sleep(INTERVAL)=>{},_=status.wake.notified()=>{}}
        }
    });
}

#[cfg(test)]
mod tests;

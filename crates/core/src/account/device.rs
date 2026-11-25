use serde::Serialize;

/// Every device token starts with this, so a caller can skip lookups for other credentials.
pub const DEVICE_TOKEN_PREFIX: &str = "cairn-device-";

/// Device use is recorded at most this often, so read-only calls do not churn rows.
pub const LAST_SEEN_THROTTLE_SECONDS: i64 = 60;

/// A device whose last hook check-in is older than this is "not seen recently".
pub const DEVICE_STALE_SECONDS: i64 = 7 * 24 * 60 * 60;

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Active,
    Revoked,
}

impl DeviceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Device {
    pub device_id: String,
    pub person_id: String,
    pub name: String,
    pub status: DeviceStatus,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_at: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub last_seen_at: i64,
    /// The host agent, from the client's user agent.
    pub agent: Option<String>,
    pub client_version: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_hook_at: Option<i64>,
    /// The retired Python hook (user agent `Python-urllib`).
    #[serde(skip)]
    pub legacy_hook: bool,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceHealth {
    Working,
    UpdateNeeded,
    Stale,
    NeverConnected,
}

/// What the console shows next to a device: a state, why, and the command that fixes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthReport {
    pub health: DeviceHealth,
    pub reason: Option<String>,
    pub fix_command: Option<String>,
}

/// `None` for a revoked device. A device has connected once a client identified itself or the
/// retired hook called. Update needed (a client older than the server, or the retired hook) wins
/// over not seen recently, because it is the one a person can act on.
pub fn device_health(
    device: &Device,
    server_version: &str,
    public_url: &str,
    now: i64,
) -> Option<HealthReport> {
    if device.status == DeviceStatus::Revoked {
        return None;
    }
    let agent = device.agent.as_deref().unwrap_or("claude");
    let setup = format!("cairn setup {public_url} --agent {agent}");
    let connected = device.client_version.is_some() || device.legacy_hook;
    if !connected {
        return Some(HealthReport {
            health: DeviceHealth::NeverConnected,
            reason: Some("No agent has connected from this device yet.".into()),
            fix_command: Some(setup),
        });
    }
    if device.legacy_hook {
        return Some(HealthReport {
            health: DeviceHealth::UpdateNeeded,
            reason: Some("This machine still runs the retired Python hook.".into()),
            fix_command: Some(setup),
        });
    }
    if let Some(version) = device
        .client_version
        .as_deref()
        .filter(|v| version_older(v, server_version))
    {
        return Some(HealthReport {
            health: DeviceHealth::UpdateNeeded,
            reason: Some(format!(
                "{} runs client {version}; the server is {server_version}.",
                device.name
            )),
            fix_command: Some("cairn update".into()),
        });
    }
    let last_checkin = device.last_hook_at.unwrap_or(device.last_seen_at);
    if now - last_checkin > DEVICE_STALE_SECONDS {
        return Some(HealthReport {
            health: DeviceHealth::Stale,
            reason: Some("No check-in for more than 7 days.".into()),
            fix_command: None,
        });
    }
    Some(HealthReport {
        health: DeviceHealth::Working,
        reason: None,
        fix_command: None,
    })
}

/// True when dotted numeric version `a` is lower than `b`. A version that does not parse is
/// never older, so a strange client is not nagged.
pub fn version_older(a: &str, b: &str) -> bool {
    fn parts(version: &str) -> Option<Vec<u64>> {
        version
            .split(['-', '+'])
            .next()?
            .split('.')
            .map(|p| p.parse().ok())
            .collect()
    }
    match (parts(a), parts(b)) {
        (Some(a), Some(b)) => a < b,
        _ => false,
    }
}

/// How the client says who it is: `User-Agent: cairn/<version> (<agent>; <kind>)`. The agent
/// is optional (an MCP server does not know its host).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientKind {
    Hook,
    Mcp,
    Cli,
}

impl ClientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hook => "hook",
            Self::Mcp => "mcp",
            Self::Cli => "cli",
        }
    }
}

pub const AGENTS: [&str; 5] = ["claude", "codex", "opencode", "grok", "antigravity"];

pub fn client_user_agent(version: &str, agent: &str, kind: ClientKind) -> String {
    if AGENTS.contains(&agent) {
        format!("cairn/{version} ({agent}; {})", kind.as_str())
    } else {
        format!("cairn/{version} ({})", kind.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInfo {
    pub version: Option<String>,
    pub agent: Option<String>,
    pub kind: Option<ClientKind>,
    /// A `Python-urllib` agent string: the retired hook.
    pub legacy: bool,
}

/// `None` for any user agent that is not a Cairn client.
pub fn parse_user_agent(user_agent: &str) -> Option<ClientInfo> {
    let user_agent = user_agent.trim();
    if user_agent.starts_with("Python-urllib") {
        return Some(ClientInfo {
            version: None,
            agent: None,
            kind: None,
            legacy: true,
        });
    }
    let rest = user_agent.strip_prefix("cairn/")?;
    let (version, tail) = rest.split_once(' ').unwrap_or((rest, ""));
    let version = version.trim();
    if version.is_empty()
        || version.len() > 40
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
    {
        return None;
    }
    let (mut agent, mut kind) = (None, None);
    for token in tail
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(';')
        .map(str::trim)
    {
        match token {
            "hook" => kind = Some(ClientKind::Hook),
            "mcp" => kind = Some(ClientKind::Mcp),
            "cli" => kind = Some(ClientKind::Cli),
            other if AGENTS.contains(&other) => agent = Some(other.to_owned()),
            _ => {}
        }
    }
    Some(ClientInfo {
        version: Some(version.to_owned()),
        agent,
        kind,
        legacy: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device {
            device_id: "dev_1".into(),
            person_id: "p".into(),
            name: "laptop".into(),
            status: DeviceStatus::Active,
            created_at: 0,
            last_seen_at: 1_000,
            agent: Some("codex".into()),
            client_version: Some("0.1.6".into()),
            last_hook_at: Some(1_000),
            legacy_hook: false,
        }
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(version_older("0.1.4", "0.1.6"));
        assert!(version_older("0.1.9", "0.1.10"));
        assert!(version_older("0.9.0", "1.0.0"));
        assert!(!version_older("0.1.6", "0.1.6"));
        assert!(!version_older("0.2.0", "0.1.6"));
        assert!(!version_older("garbage", "0.1.6"));
        assert!(!version_older("0.1.6-rc1", "0.1.6"));
    }

    #[test]
    fn user_agents_round_trip() {
        for agent in AGENTS {
            let hook =
                parse_user_agent(&client_user_agent("0.1.6", agent, ClientKind::Hook)).unwrap();
            assert_eq!(
                (hook.version.as_deref(), hook.agent.as_deref(), hook.kind),
                (Some("0.1.6"), Some(agent), Some(ClientKind::Hook))
            );
        }
        let mcp = parse_user_agent(&client_user_agent("0.1.6", "", ClientKind::Mcp)).unwrap();
        assert_eq!((mcp.agent, mcp.kind), (None, Some(ClientKind::Mcp)));
        assert!(parse_user_agent("Python-urllib/3.12").unwrap().legacy);
        assert!(parse_user_agent("curl/8.0").is_none());
        assert!(parse_user_agent("cairn/").is_none());
        assert!(parse_user_agent("cairn/<script>").is_none());
    }

    #[test]
    fn health_states_in_priority_order() {
        let now = 1_000 + 60;
        let report = |d: &Device| device_health(d, "0.1.6", "https://c.example", now);
        assert_eq!(report(&device()).unwrap().health, DeviceHealth::Working);
        let mut never = device();
        (never.client_version, never.last_hook_at) = (None, None);
        let never = report(&never).unwrap();
        assert_eq!(never.health, DeviceHealth::NeverConnected);
        assert_eq!(
            never.fix_command.as_deref(),
            Some("cairn setup https://c.example --agent codex")
        );
        let mut old = device();
        old.client_version = Some("0.1.4".into());
        old.last_hook_at = Some(-DEVICE_STALE_SECONDS * 2);
        let old = report(&old).unwrap();
        assert_eq!(old.health, DeviceHealth::UpdateNeeded, "update beats stale");
        assert_eq!(old.fix_command.as_deref(), Some("cairn update"));
        let mut legacy = device();
        (legacy.legacy_hook, legacy.client_version) = (true, None);
        assert_eq!(report(&legacy).unwrap().health, DeviceHealth::UpdateNeeded);
        let mut quiet = device();
        quiet.last_hook_at = Some(now - DEVICE_STALE_SECONDS - 1);
        assert_eq!(report(&quiet).unwrap().health, DeviceHealth::Stale);
        let mut revoked = device();
        revoked.status = DeviceStatus::Revoked;
        assert!(report(&revoked).is_none());
    }
}

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct IrcMessage {
    pub tags: HashMap<String, String>,
    pub prefix: Option<String>,
    pub command: String,
    pub params: Vec<String>,
}

impl IrcMessage {
    pub fn parse(line: &str) -> Option<Self> {
        let mut rest = line.trim_end_matches(['\r', '\n']);
        if rest.is_empty() {
            return None;
        }

        let mut tags = HashMap::new();
        if let Some(stripped) = rest.strip_prefix('@') {
            let (raw_tags, remainder) = stripped.split_once(' ')?;
            for pair in raw_tags.split(';') {
                if pair.is_empty() {
                    continue;
                }
                match pair.split_once('=') {
                    Some((k, v)) => tags.insert(k.to_string(), unescape_tag(v)),
                    None => tags.insert(pair.to_string(), String::new()),
                };
            }
            rest = remainder;
        }

        let mut prefix = None;
        if let Some(stripped) = rest.strip_prefix(':') {
            let (pfx, remainder) = stripped.split_once(' ')?;
            prefix = Some(pfx.to_string());
            rest = remainder;
        }

        let (command, mut rest) = match rest.split_once(' ') {
            Some((cmd, remainder)) => (cmd.to_string(), remainder),
            None => (rest.to_string(), ""),
        };

        let mut params = Vec::new();
        loop {
            rest = rest.trim_start_matches(' ');
            if rest.is_empty() {
                break;
            }
            if let Some(trailing) = rest.strip_prefix(':') {
                params.push(trailing.to_string());
                break;
            }
            match rest.split_once(' ') {
                Some((param, remainder)) => {
                    params.push(param.to_string());
                    rest = remainder;
                }
                None => {
                    params.push(rest.to_string());
                    break;
                }
            }
        }

        Some(Self {
            tags,
            prefix,
            command,
            params,
        })
    }

    pub fn sender_login(&self) -> Option<&str> {
        if let Some(login) = self.tags.get("login") {
            return Some(login.as_str());
        }
        self.prefix.as_deref().and_then(|p| p.split('!').next())
    }

    pub fn display_name(&self) -> Option<&str> {
        self.tags
            .get("display-name")
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| self.sender_login())
    }

    pub fn channel(&self) -> Option<&str> {
        self.params
            .first()
            .filter(|p| p.starts_with('#'))
            .map(|p| p.trim_start_matches('#'))
    }

    pub fn text(&self) -> Option<&str> {
        self.params.get(1).map(|s| s.as_str())
    }

    pub fn is_moderator(&self) -> bool {
        self.tags.get("mod").map(|v| v == "1").unwrap_or(false)
            || self
                .tags
                .get("badges")
                .map(|b| b.contains("broadcaster/") || b.contains("moderator/"))
                .unwrap_or(false)
    }
}

fn unescape_tag(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(':') => out.push(';'),
                Some('s') => out.push(' '),
                Some('r') => out.push('\r'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_privmsg_with_tags() {
        let line = "@badges=broadcaster/1;display-name=Anna;mod=0 :anna!anna@anna.tmi.twitch.tv PRIVMSG #chan :hello world";
        let msg = IrcMessage::parse(line).unwrap();
        assert_eq!(msg.command, "PRIVMSG");
        assert_eq!(msg.channel(), Some("chan"));
        assert_eq!(msg.text(), Some("hello world"));
        assert_eq!(msg.display_name(), Some("Anna"));
        assert_eq!(msg.sender_login(), Some("anna"));
        assert!(msg.is_moderator());
    }

    #[test]
    fn parses_ping() {
        let msg = IrcMessage::parse("PING :tmi.twitch.tv").unwrap();
        assert_eq!(msg.command, "PING");
        assert_eq!(msg.params, vec!["tmi.twitch.tv".to_string()]);
    }
}

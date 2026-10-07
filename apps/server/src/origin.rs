pub fn validate(value: &str, allow_insecure_lan: bool) -> anyhow::Result<url::Url> {
    let parsed = url::Url::parse(value)?;
    anyhow::ensure!(
        parsed.origin().ascii_serialization() == value,
        "origin 必须为不带路径的完整 origin"
    );
    let local = match parsed.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || (allow_insecure_lan && ip.is_private()),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    anyhow::ensure!(
        parsed.scheme() == "https" || (parsed.scheme() == "http" && local),
        "非本机 origin 必须使用 HTTPS；私有 IPv4 局域网可显式使用 --allow-insecure-lan"
    );
    Ok(parsed)
}

pub fn request_allowed(allow_any: bool, origin: Option<&str>, allowed: &[String]) -> bool {
    allow_any || origin.is_some_and(|origin| allowed.iter().any(|entry| entry == origin))
}

#[cfg(test)]
mod tests {
    use super::{request_allowed, validate};
    #[test]
    fn any_origin_requires_explicit_opt_in() {
        let allowed = vec!["http://127.0.0.1:7777".to_owned()];
        let tailscale = Some("http://100.104.72.32:7777");
        assert!(!request_allowed(false, tailscale, &allowed));
        assert!(request_allowed(true, tailscale, &allowed));
        assert!(!request_allowed(false, None, &allowed));
        assert!(request_allowed(false, Some(&allowed[0]), &allowed));
    }
    #[test]
    fn lan_http_is_explicit_and_does_not_allow_public_hosts() {
        assert!(validate("http://192.168.2.174:7777", false).is_err());
        assert!(validate("http://192.168.2.174:7777", true).is_ok());
        assert!(validate("http://127.0.0.1:7777", false).is_ok());
        assert!(validate("http://8.8.8.8:7777", true).is_err());
        assert!(validate("http://example.com", true).is_err());
        assert!(validate("http://0.0.0.0:7777", true).is_err());
        assert!(validate("http://192.168.2.174:7777/path", true).is_err());
    }
}

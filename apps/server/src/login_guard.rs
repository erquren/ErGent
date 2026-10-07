use crate::ApiError;
use axum::http::{HeaderMap, StatusCode};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    net::IpAddr,
};

pub fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        _ => ip,
    }
}

pub fn invalid_login() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
        "账号或密码错误",
    )
}

pub fn client_ip(
    peer: IpAddr,
    headers: &HeaderMap,
    trusted: &[IpAddr],
) -> Result<IpAddr, ApiError> {
    let peer = normalize(peer);
    if !trusted.contains(&peer) {
        return Ok(peer); // Never trust arbitrary clients' forwarding headers.
    }
    let mut values = headers.get_all("x-real-ip").iter();
    let value = values.next().and_then(|v| v.to_str().ok());
    if values.next().is_some() {
        return Err(invalid_login());
    }
    value
        .and_then(|v| v.parse::<IpAddr>().ok())
        .map(normalize)
        .ok_or_else(invalid_login)
}

pub fn stripe(ip: IpAddr) -> usize {
    let mut hash = DefaultHasher::new();
    ip.hash(&mut hash);
    hash.finish() as usize % 64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwarding_requires_explicit_trust_and_a_single_valid_address() {
        let peer = "127.0.0.1".parse().unwrap();
        let mut h = HeaderMap::new();
        h.insert("x-real-ip", "203.0.113.5".parse().unwrap());
        h.insert("x-forwarded-for", "198.51.100.1".parse().unwrap());
        assert_eq!(client_ip(peer, &h, &[]).unwrap(), peer);
        assert_eq!(
            client_ip(peer, &h, &[peer]).unwrap(),
            "203.0.113.5".parse::<IpAddr>().unwrap()
        );
        h.append("x-real-ip", "203.0.113.6".parse().unwrap());
        assert!(client_ip(peer, &h, &[peer]).is_err());
        for value in [
            "",
            "garbage",
            "203.0.113.5, 203.0.113.6",
            "203.0.113.5:1234",
        ] {
            h.remove("x-real-ip");
            h.insert("x-real-ip", value.parse().unwrap());
            assert!(client_ip(peer, &h, &[peer]).is_err());
        }
        assert!(client_ip(peer, &HeaderMap::new(), &[peer]).is_err());
        assert_eq!(
            normalize("::ffff:192.0.2.1".parse().unwrap()),
            "192.0.2.1".parse::<IpAddr>().unwrap()
        );
    }
}

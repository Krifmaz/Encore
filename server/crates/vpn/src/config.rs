//! The wg-quick .conf file a VPN provider hands out (ProtonVPN: Downloads → WireGuard configuration).
//! Only what a single-peer client needs; IPv6 addresses are skipped (the tunnel is IPv4 only).
use base64::Engine as _;
use std::net::{IpAddr, Ipv4Addr};

/// A parsed WireGuard config: the parts the tunnel needs.
#[derive(Clone, Debug, PartialEq)]
pub struct WgConfig {
    /// our private key (Interface PrivateKey)
    pub private_key: [u8; 32],
    /// The tunnel address (first IPv4 one in Address).
    pub address: Ipv4Addr,
    /// the tunnel address's prefix length (the /n in Address)
    pub prefix: u8,
    /// DNS servers, IPv4 ones first.
    pub dns: Vec<IpAddr>,
    /// the server's public key (Peer PublicKey)
    pub public_key: [u8; 32],
    /// the extra pre-shared key, if the config has one
    pub preshared_key: Option<[u8; 32]>,
    /// host:port as written; the host may be a name.
    pub endpoint_host: String,
    /// the server's port
    pub endpoint_port: u16,
    /// the AllowedIPs entries as written
    pub allowed_ips: Vec<String>,
    /// seconds between keepalives, 0 for none (PersistentKeepalive)
    pub keepalive: u16,
}

impl WgConfig {
    /// The first IPv4 DNS server: Proton's resolver and NAT-PMP gateway (10.2.0.1).
    pub fn dns_v4(&self) -> Option<Ipv4Addr> {
        self.dns.iter().find_map(|d| match d { IpAddr::V4(v) => Some(*v), _ => None })
    }

    /// The endpoint as `host:port`, with an IPv6 host in brackets.
    pub fn endpoint(&self) -> String {
        if self.endpoint_host.contains(':') { format!("[{}]:{}", self.endpoint_host, self.endpoint_port) } else { format!("{}:{}", self.endpoint_host, self.endpoint_port) }
    }
}

/// Decode one base64 key; `name` names it in the error.
fn key(v: &str, name: &str) -> Result<[u8; 32], String> {
    let b = base64::engine::general_purpose::STANDARD.decode(v.trim()).map_err(|_| format!("{name} in the WireGuard config isn't a valid key (it should be 44 characters of base64)"))?;
    b.try_into().map_err(|_| format!("{name} in the WireGuard config isn't a valid key (it should be 44 characters of base64)"))
}

/// Parse a wg-quick config. Tolerant of comments (# and ;), blank lines, spaces, CRLF and any key case.
pub fn parse(text: &str) -> Result<WgConfig, String> {
    if text.trim().is_empty() {
        return Err("No WireGuard config yet – paste the .conf file from your VPN provider".into());
    }
    // an environment variable may carry the file on one line with literal \n
    let unescaped;
    let text = if !text.contains('\n') && text.contains("\\n") {
        unescaped = text.replace("\\r\\n", "\n").replace("\\n", "\n");
        unescaped.as_str()
    } else {
        text
    };
    let mut section = String::new();
    let mut seen_iface = false;
    let mut seen_peer = 0;
    let (mut private_key, mut address, mut public_key, mut endpoint) = (None, None, None, None);
    let (mut preshared_key, mut keepalive) = (None, None);
    let mut had_v6_only = false;
    let mut dns: Vec<IpAddr> = vec![];
    let mut allowed_ips: Vec<String> = vec![];
    for raw in text.lines() {
        let line = raw.split(['#', ';']).next().unwrap_or("").trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_lowercase();
            if section == "interface" { seen_iface = true; }
            if section == "peer" { seen_peer += 1; }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim().to_lowercase(), v.trim());
        match (section.as_str(), k.as_str()) {
            ("interface", "privatekey") => private_key = Some(key(v, "PrivateKey")?),
            ("interface", "address") => {
                for a in v.split(',').map(str::trim).filter(|a| !a.is_empty()) {
                    let (ip, prefix) = a.split_once('/').unwrap_or((a, "32"));
                    match ip.trim().parse::<IpAddr>() {
                        Ok(IpAddr::V4(ip)) => {
                            if address.is_none() {
                                let p: u8 = prefix.trim().parse().ok().filter(|p| *p <= 32).ok_or_else(|| format!("Address {a} in the WireGuard config has a bad prefix"))?;
                                address = Some((ip, p));
                            }
                        }
                        Ok(IpAddr::V6(_)) => had_v6_only = true,
                        Err(_) => return Err(format!("Address {a} in the WireGuard config isn't an IP address")),
                    }
                }
            }
            ("interface", "dns") => {
                // DNS may also list search domains; only addresses count
                dns.extend(v.split(',').filter_map(|d| d.trim().parse::<IpAddr>().ok()));
            }
            ("peer", "publickey") if seen_peer == 1 => public_key = Some(key(v, "PublicKey")?),
            ("peer", "presharedkey") if seen_peer == 1 => preshared_key = Some(key(v, "PresharedKey")?),
            ("peer", "endpoint") if seen_peer == 1 => {
                let (host, port) = if let Some(rest) = v.strip_prefix('[') {
                    let (h, p) = rest.split_once("]:").ok_or_else(|| format!("Endpoint {v} in the WireGuard config should be host:port"))?;
                    (h.to_string(), p)
                } else {
                    let (h, p) = v.rsplit_once(':').ok_or_else(|| format!("Endpoint {v} in the WireGuard config should be host:port"))?;
                    (h.trim().to_string(), p)
                };
                let port: u16 = port.trim().parse().ok().filter(|p| *p > 0).ok_or_else(|| format!("Endpoint {v} in the WireGuard config has a bad port"))?;
                if host.is_empty() {
                    return Err(format!("Endpoint {v} in the WireGuard config has no host"));
                }
                endpoint = Some((host, port));
            }
            ("peer", "allowedips") if seen_peer == 1 => allowed_ips.extend(v.split(',').map(|a| a.trim().to_string()).filter(|a| !a.is_empty())),
            ("peer", "persistentkeepalive") if seen_peer == 1 => {
                keepalive = Some(if v.eq_ignore_ascii_case("off") { 0 } else { v.parse::<u16>().map_err(|_| format!("PersistentKeepalive {v} in the WireGuard config isn't a number"))? })
            }
            _ => {}
        }
    }
    if !seen_iface {
        return Err("The WireGuard config has no [Interface] section".into());
    }
    if seen_peer == 0 {
        return Err("The WireGuard config has no [Peer] section".into());
    }
    let private_key = private_key.ok_or("The WireGuard config is missing PrivateKey in [Interface]")?;
    let (address, prefix) = address.ok_or(if had_v6_only { "The WireGuard config has only IPv6 addresses – an IPv4 Address is needed in [Interface]" } else { "The WireGuard config is missing Address in [Interface]" })?;
    let public_key = public_key.ok_or("The WireGuard config is missing PublicKey in [Peer]")?;
    let (endpoint_host, endpoint_port) = endpoint.ok_or("The WireGuard config is missing Endpoint in [Peer]")?;
    dns.sort_by_key(|d| !d.is_ipv4());
    Ok(WgConfig { private_key, address, prefix, dns, public_key, preshared_key, endpoint_host, endpoint_port, allowed_ips, keepalive: keepalive.unwrap_or(25) })
}

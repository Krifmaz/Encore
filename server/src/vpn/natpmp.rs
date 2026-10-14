//! NAT-PMP (RFC 6886), the way ProtonVPN forwards a port: ask the gateway (10.2.0.1) every 45 s for a
//! 60 s mapping, UDP and TCP, internal port 0 and suggested external port 1; the answer's external
//! port is the forwarded one (Proton maps the same port number inside).
use std::net::Ipv4Addr;

pub const PORT: u16 = 5351;
pub const OP_ADDRESS: u8 = 0;
pub const OP_UDP: u8 = 1;
pub const OP_TCP: u8 = 2;

/// Request the public address (opcode 0).
pub fn address_request() -> Vec<u8> {
    vec![0, OP_ADDRESS]
}

/// Request a mapping (opcode 1 = UDP, 2 = TCP).
pub fn map_request(op: u8, internal: u16, external: u16, lifetime: u32) -> Vec<u8> {
    let mut out = vec![0, op, 0, 0];
    out.extend_from_slice(&internal.to_be_bytes());
    out.extend_from_slice(&external.to_be_bytes());
    out.extend_from_slice(&lifetime.to_be_bytes());
    out
}

fn result_text(code: u16) -> String {
    match code {
        1 => "the VPN server doesn't support this NAT-PMP version".into(),
        2 => "port forwarding is refused – turn on NAT-PMP / port forwarding for this WireGuard config in your VPN account".into(),
        3 => "the VPN server's network is down".into(),
        4 => "the VPN server is out of ports".into(),
        5 => "the VPN server doesn't know this NAT-PMP request".into(),
        c => format!("NAT-PMP error {c}"),
    }
}

fn header(p: &[u8], op: u8, len: usize) -> Result<(), String> {
    if p.len() < 4 || p[0] != 0 || p[1] != 128 + op {
        return Err("not a NAT-PMP answer".into());
    }
    let code = u16::from_be_bytes([p[2], p[3]]);
    if code != 0 {
        return Err(result_text(code));
    }
    if p.len() < len {
        return Err("short NAT-PMP answer".into());
    }
    Ok(())
}

/// The public address from an opcode 0 answer.
pub fn parse_address(p: &[u8]) -> Result<Ipv4Addr, String> {
    header(p, OP_ADDRESS, 12)?;
    Ok(Ipv4Addr::new(p[8], p[9], p[10], p[11]))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    pub internal: u16,
    pub external: u16,
    pub lifetime: u32,
}

/// A mapping answer for opcode `op`.
pub fn parse_map(p: &[u8], op: u8) -> Result<Mapping, String> {
    header(p, op, 16)?;
    Ok(Mapping { internal: u16::from_be_bytes([p[8], p[9]]), external: u16::from_be_bytes([p[10], p[11]]), lifetime: u32::from_be_bytes([p[12], p[13], p[14], p[15]]) })
}

/// The gateway's side (used by the tests' NAT-PMP stub): answer `req` with `port` / `public`.
#[cfg(test)]
pub fn answer(req: &[u8], port: u16, public: Ipv4Addr) -> Vec<u8> {
    let op = req[1];
    let mut out = vec![0, 128 + op, 0, 0, 0, 0, 0, 7];
    if op == OP_ADDRESS {
        out.extend_from_slice(&public.octets());
    } else {
        out.extend_from_slice(&port.to_be_bytes());
        out.extend_from_slice(&port.to_be_bytes());
        out.extend_from_slice(&req[8..12]);
    }
    out
}

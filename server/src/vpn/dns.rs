//! Just enough DNS to look up A records over the tunnel (the Soulseek server's name).
use std::net::Ipv4Addr;

/// A standard query for the A records of `host`, recursion desired.
pub fn query(id: u16, host: &str) -> Result<Vec<u8>, String> {
    let host = host.trim().trim_end_matches('.');
    let mut out = Vec::with_capacity(18 + host.len());
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // RD; 1 question
    if host.is_empty() {
        return Err("empty host name".into());
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(format!("{host} isn't a valid host name"));
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&[0, 1, 0, 1]); // A, IN
    Ok(out)
}

fn skip_name(p: &[u8], mut i: usize) -> Option<usize> {
    loop {
        let len = *p.get(i)? as usize;
        if len == 0 {
            return Some(i + 1);
        }
        if len & 0xc0 == 0xc0 {
            p.get(i + 1)?;
            return Some(i + 2);
        }
        i += 1 + len;
    }
}

/// The A records (and the shortest TTL) of an answer to our query `id`.
pub fn parse(p: &[u8], id: u16) -> Result<(Vec<Ipv4Addr>, u32), String> {
    if p.len() < 12 {
        return Err("short DNS answer".into());
    }
    if u16::from_be_bytes([p[0], p[1]]) != id {
        return Err("DNS answer for another query".into());
    }
    if p[2] & 0x80 == 0 {
        return Err("not a DNS answer".into());
    }
    let rcode = p[3] & 0x0f;
    if rcode == 3 {
        return Err("no such host name".into());
    }
    if rcode != 0 {
        return Err(format!("the DNS server answered with error {rcode}"));
    }
    let qd = u16::from_be_bytes([p[4], p[5]]) as usize;
    let an = u16::from_be_bytes([p[6], p[7]]) as usize;
    let mut i = 12;
    for _ in 0..qd {
        i = skip_name(p, i).ok_or("bad DNS answer")? + 4;
    }
    let mut ips = vec![];
    let mut ttl = u32::MAX;
    for _ in 0..an {
        i = skip_name(p, i).ok_or("bad DNS answer")?;
        let h = p.get(i..i + 10).ok_or("bad DNS answer")?;
        let (typ, class) = (u16::from_be_bytes([h[0], h[1]]), u16::from_be_bytes([h[2], h[3]]));
        let t = u32::from_be_bytes([h[4], h[5], h[6], h[7]]);
        let len = u16::from_be_bytes([h[8], h[9]]) as usize;
        let data = p.get(i + 10..i + 10 + len).ok_or("bad DNS answer")?;
        if typ == 1 && class == 1 && len == 4 {
            ips.push(Ipv4Addr::new(data[0], data[1], data[2], data[3]));
            ttl = ttl.min(t);
        }
        i += 10 + len;
    }
    if ips.is_empty() {
        return Err("the host name has no IPv4 address".into());
    }
    Ok((ips, ttl))
}

/// The other side: answer `q` with `ips` (used by the tests' DNS stub).
#[cfg(test)]
pub fn answer(q: &[u8], ips: &[Ipv4Addr]) -> Vec<u8> {
    let end = skip_name(q, 12).unwrap() + 4;
    let mut out = q[..end].to_vec();
    out[2] = 0x81;
    out[3] = 0x80;
    out[6..8].copy_from_slice(&(ips.len() as u16).to_be_bytes());
    for ip in ips {
        out.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1, 0, 0, 0x0e, 0x10, 0, 4]);
        out.extend_from_slice(&ip.octets());
    }
    out
}

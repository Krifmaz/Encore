//! Outgoing email over the admin's SMTP server (password resets, sign-in links, test mail).
use crate::state::AppState;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

pub fn enabled(st: &AppState) -> bool {
    !st.cfg.str("smtp_host").is_empty()
}

pub async fn send(st: &AppState, to: &str, subject: &str, text: &str) -> anyhow::Result<()> {
    if !enabled(st) {
        anyhow::bail!("Email isn't set up – add an SMTP server under Settings → Email & notifications");
    }
    let host = st.cfg.str("smtp_host");
    let port = st.cfg.int("smtp_port");
    let user = st.cfg.str("smtp_user");
    let from = { let f = st.cfg.str("smtp_from"); if f.is_empty() { user.clone() } else { f } };
    let from: Mailbox = from.parse().map_err(|e| anyhow::anyhow!("From address: {e}"))?;
    let msg = Message::builder().from(from).to(to.parse().map_err(|e| anyhow::anyhow!("To address: {e}"))?).subject(subject).body(text.to_string())?;
    let builder = match st.cfg.str("smtp_security").as_str() {
        "ssl" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host)?,
        "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&host),
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)?,
    };
    let mut builder = builder.port(if port > 0 { port as u16 } else { 587 }).timeout(Some(std::time::Duration::from_secs(20)));
    if !user.is_empty() {
        builder = builder.credentials(Credentials::new(user, st.cfg.str("smtp_password")));
    }
    builder.build().send(msg).await?;
    Ok(())
}
